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
use rustx::events::interaction::{
    CustomAnswer, InteractionSettlement, InteractionSubject, MAX_APPROVAL_REQUEST_REASON_CHARS,
    MAX_OPTION_LABEL_CHARS, MAX_QUESTION_TEXT_CHARS, MAX_QUESTIONNAIRE_QUESTIONS,
    MultipleOptionAnswer, OptionSpecification, QuestionSpecification, QuestionnaireAnswer,
    QuestionnaireAnswerEntry, QuestionnaireSpecification, QuestionnaireSubmission,
    SingleOptionAnswer, interaction_arguments_digest,
};
use rustx::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
};
use rustx::model::catalog::{ModelCapabilities, ModelCompat};
use rustx::model::{
    ModelFinishReason, ModelInvocationConfig, ModelProtocol, RequestIdentity, RequestParams,
    RequestSnapshot,
};
use rustx::publication::{PublicationFrame, PublicationPayload, PublicationStreamStart};
use rustx::runtime::ApprovalDecision;
use rustx::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, EventId, InteractionId, MessageId,
    PublicationStreamId, RequestId, ToolCallId, ToolId, TurnId,
};
use rustx::runtime::recovery::{AttemptRecoveryClass, RecoveryReport, recover};
use rustx::runtime::types::{CancellationReason, RuntimeClock};
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

/// The exact model-issued arguments the canonical `ToolCall` of
/// [`commit_turn_up_to_the_policy_boundary`] freezes.
fn canonical_arguments() -> serde_json::Value {
    serde_json::json!({"path": "notes.md", "limit": 20})
}

/// The one approval subject that truthfully describes that canonical call.
fn approval_subject() -> InteractionSubject {
    InteractionSubject::Approval {
        call_id: call_id(),
        tool_id: ToolId::new("tool-alpha"),
        tool_name: "alpha".to_owned(),
        arguments_digest: interaction_arguments_digest(&canonical_arguments()),
        reason: "the policy asked".to_owned(),
    }
}

fn questionnaire_specification() -> QuestionnaireSpecification {
    QuestionnaireSpecification {
        questions: vec![QuestionSpecification {
            question: "Which target?".to_owned(),
            header: "Target".to_owned(),
            options: vec![
                OptionSpecification {
                    label: "staging".to_owned(),
                    description: "A safe test environment.".to_owned(),
                    preview: None,
                },
                OptionSpecification {
                    label: "production".to_owned(),
                    description: "The live environment.".to_owned(),
                    preview: None,
                },
            ],
            multi_select: false,
        }],
    }
}

fn questionnaire_subject() -> InteractionSubject {
    InteractionSubject::Questionnaire {
        questionnaire: questionnaire_specification(),
    }
}

fn submitted_option(label: &str) -> InteractionSettlement {
    InteractionSettlement::QuestionnaireSubmitted {
        submission: QuestionnaireSubmission {
            answers: vec![QuestionnaireAnswerEntry {
                question_index: 0,
                answer: QuestionnaireAnswer::SingleOption(SingleOptionAnswer {
                    label: label.to_owned(),
                }),
            }],
        },
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
fn start_request(
    store: &SqliteConversationStore,
    attempt_id: &AttemptId,
    turn: &TurnId,
) -> RequestId {
    let head = store.load_head().expect("head");
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt_id.clone(),
            turn: turn.clone(),
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

/// One complete durable model-turn **generation**: the attempt and turn that
/// own it, and the exact canonical Assistant `ToolCall` its publication stream
/// froze and its Assistant message committed.
///
/// The interaction regressions exist to prove that an Approval audit fact is
/// bound to one of these, not merely to a call identity that happens to be
/// findable somewhere in canonical history.
#[derive(Clone)]
struct Generation {
    attempt: AttemptId,
    turn: TurnId,
    call: ToolCall,
}

impl Generation {
    /// The generation every default fixture builds: `attempt-1`, turn 1, one
    /// canonical `call-approved` proposing `tool-alpha`/`alpha`.
    fn first() -> Self {
        Self {
            attempt: attempt(),
            turn: TurnId::new("1"),
            call: ToolCall {
                id: call_id(),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
                arguments: canonical_arguments(),
            },
        }
    }

    /// A later generation of the same or another attempt, proposing its own
    /// distinct canonical call.
    fn next(attempt: &str, turn: &str, call: &str) -> Self {
        Self {
            attempt: AttemptId::new(attempt),
            turn: TurnId::new(turn),
            call: ToolCall {
                id: ToolCallId::new(call),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
                arguments: canonical_arguments(),
            },
        }
    }

    fn message_id(&self) -> MessageId {
        MessageId::new(format!("{}-agent-{}", self.attempt, self.turn))
    }

    /// An audit envelope pinned to exactly this generation.
    fn envelope(&self, event_id: &str, event: RuntimeEvent) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            attempt_id: Some(self.attempt.clone()),
            turn_id: Some(self.turn.clone()),
            ..envelope(event_id, event)
        }
    }

    fn requested(
        &self,
        interaction_id: &InteractionId,
        subject: InteractionSubject,
    ) -> RuntimeEventEnvelope {
        self.envelope(
            &requested_event_id(interaction_id),
            RuntimeEvent::InteractionRequested {
                interaction_id: interaction_id.clone(),
                subject,
            },
        )
    }

    fn settled(
        &self,
        interaction_id: &InteractionId,
        settlement: InteractionSettlement,
    ) -> RuntimeEventEnvelope {
        self.envelope(
            &settled_event_id(interaction_id),
            RuntimeEvent::InteractionSettled {
                interaction_id: interaction_id.clone(),
                settlement,
            },
        )
    }

    /// The one approval subject that truthfully describes this generation's
    /// canonical call.
    fn approval_subject(&self) -> InteractionSubject {
        InteractionSubject::Approval {
            call_id: self.call.id.clone(),
            tool_id: self.call.tool_id.clone(),
            tool_name: self.call.name.clone(),
            arguments_digest: interaction_arguments_digest(&self.call.arguments),
            reason: "the policy asked".to_owned(),
        }
    }
}

/// Commits the exact durable prefix a real attempt reaches immediately before
/// the pre-tool policy boundary: an attempt start (for a new attempt), one
/// complete provider generation, and one canonical Assistant turn proposing
/// this generation's call.
///
/// Nothing here is an execution authorization: no `ToolExecutionStarted`
/// exists, which is precisely the state the interaction regressions build on.
fn commit_generation(store: &SqliteConversationStore, generation: &Generation) {
    let request_id = start_request(store, &generation.attempt, &generation.turn);
    let message_id = generation.message_id();
    let start = PublicationStreamStart {
        stream_id: PublicationStreamId::for_request(&generation.attempt, &message_id),
        attempt_id: generation.attempt.clone(),
        turn_id: generation.turn.clone(),
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
                    id: generation.call.id.clone(),
                    tool_id: generation.call.tool_id.clone(),
                    name: generation.call.name.clone(),
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
                call: generation.call.clone(),
            },
        )])
        .expect("proposal complete");
    store
        .append_event(generation.envelope(
            &format!(
                "request-completed-{}-{}",
                generation.attempt, generation.turn
            ),
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
                content: vec![AssistantContentBlock::ToolCall(generation.call.clone())],
            }),
            generation.envelope(
                &format!(
                    "assistant-committed-{}-{}",
                    generation.attempt, generation.turn
                ),
                RuntimeEvent::AssistantMessageCommitted { message_id },
            ),
        )
        .expect("canonical Assistant");
}

/// Records the durable start of one attempt.
fn start_attempt(store: &SqliteConversationStore, attempt_id: &AttemptId) {
    store
        .append_event(RuntimeEventEnvelope {
            turn_id: None,
            attempt_id: Some(attempt_id.clone()),
            ..envelope(
                &format!("attempt-start-{attempt_id}"),
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                },
            )
        })
        .expect("attempt start");
}

fn commit_turn_up_to_the_policy_boundary(store: &SqliteConversationStore) {
    let generation = Generation::first();
    store.initialize(&[]).expect("initialize");
    start_attempt(store, &generation.attempt);
    commit_generation(store, &generation);
}

/// An in-memory conversation committed exactly up to the pre-tool policy
/// boundary. The canonical Assistant `ToolCall` that an approval audit subject
/// must match already exists, and nothing has been authorized to execute.
fn policy_boundary_store() -> SqliteConversationStore {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    commit_turn_up_to_the_policy_boundary(&store);
    store
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
    let store = policy_boundary_store();
    let id = interaction_id();

    // A settlement for an interaction that never durably existed asserts a
    // decision about a prompt no user was ever shown.
    assert!(matches!(
        store.append_interaction_audit(settled(&id, InteractionSettlement::Approved)),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    store
        .append_interaction_audit(requested(&id, approval_subject()))
        .expect("requested");
    // The identity is spent: a second requested fact would fork the audit.
    assert!(matches!(
        store.append_interaction_audit(requested(&id, approval_subject())),
        Err(ConversationStoreError::TerminalViolation(_))
    ));

    store
        .append_interaction_audit(settled(&id, InteractionSettlement::Approved))
        .expect("settled");
    assert!(matches!(
        store.append_interaction_audit(settled(&id, InteractionSettlement::Approved)),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert!(
        matches!(
            store.append_interaction_audit(settled(
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
/// produce: an Approval cannot receive questionnaire facts, and a
/// Questionnaire cannot be approved.
#[test]
fn a_settlement_must_match_the_subject_it_settles() {
    let store = policy_boundary_store();

    let approval = InteractionId::for_attempt(&attempt(), 1);
    store
        .append_interaction_audit(requested(&approval, approval_subject()))
        .expect("approval requested");
    assert!(matches!(
        store.append_interaction_audit(settled(
            &approval,
            InteractionSettlement::QuestionnaireDeclined,
        )),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    let question = InteractionId::for_attempt(&attempt(), 2);
    store
        .append_interaction_audit(requested(&question, questionnaire_subject()))
        .expect("question requested");
    assert!(matches!(
        store.append_interaction_audit(settled(&question, InteractionSettlement::Approved)),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    // Cancellation is the one terminal both subjects share.
    store
        .append_interaction_audit(settled(
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
    let store = policy_boundary_store();
    let id = interaction_id();

    let mut forged = requested(&id, approval_subject());
    forged.event_id = EventId::new("interaction-requested-event:some-other-interaction");
    assert!(matches!(
        store.append_interaction_audit(forged),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    store
        .append_interaction_audit(requested(&id, approval_subject()))
        .expect("requested");
    let mut forged_settlement = settled(&id, InteractionSettlement::Approved);
    forged_settlement.event_id = EventId::new("interaction-settled-event:some-other-interaction");
    assert!(matches!(
        store.append_interaction_audit(forged_settlement),
        Err(ConversationStoreError::InvalidReference(_))
    ));
}

/// The narrow interaction audit capability is not a general Event Journal
/// seam: it commits the two interaction facts and refuses everything else,
/// including the very facts that would authorize a side effect.
#[test]
fn the_narrow_audit_capability_commits_interaction_facts_only() {
    let store: Arc<dyn ConversationStore> = Arc::new(policy_boundary_store());
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
            .append_interaction_audit(requested(&id, approval_subject()))
            .expect("requested");
        store
            .append_interaction_audit(settled(&id, InteractionSettlement::Approved))
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
            .append_interaction_audit(requested(&id, approval_subject()))
            .expect("requested");
        store
            .append_interaction_audit(settled(&id, InteractionSettlement::Approved))
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
        store.append_interaction_audit(settled(&id, InteractionSettlement::Approved)),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert!(matches!(
        store.append_interaction_audit(requested(&id, approval_subject())),
        Err(ConversationStoreError::TerminalViolation(_))
    ));

    // The post-restart attempt cannot even *name* the historical call: the
    // approved `call-approved` belongs to attempt-1 turn 1, and an approval
    // asked by a new generation must describe that generation's own canonical
    // ToolCall. Copying the old subject verbatim is refused, which is a second,
    // independent reason the old decision cannot be recycled.
    let fresh_generation = Generation::next("attempt-2", "1", "call-post-restart");
    let fresh = InteractionId::for_attempt(&fresh_generation.attempt, 1);
    assert_ne!(fresh, id);
    assert!(
        matches!(
            store.append_interaction_audit(fresh_generation.requested(&fresh, approval_subject())),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "the new attempt cannot re-use the historical call as its own subject"
    );

    // A new live approval is an ordinary new identity in the current
    // (post-restart) attempt domain, asking about its own newly proposed call
    // and entirely disjoint from the historical one.
    start_attempt(&store, &fresh_generation.attempt);
    commit_generation(&store, &fresh_generation);
    store
        .append_interaction_audit(
            fresh_generation.requested(&fresh, fresh_generation.approval_subject()),
        )
        .expect("a new attempt asks its own new question about its own new call");

    assert_eq!(
        interaction_facts(&store).len(),
        3,
        "the historical pair is untouched and the new request is additive"
    );
    assert!(
        !has_tool_start(&store),
        "nothing about the new approval executed the historical call"
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
            .append_interaction_audit(requested(&id, questionnaire_subject()))
            .expect("requested");
    }

    let (store, report) = recover_reopened(&durable);
    let facts = interaction_facts(&store);
    assert!(
        matches!(
            facts.as_slice(),
            [RuntimeEvent::InteractionRequested {
                subject: InteractionSubject::Questionnaire { .. },
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
        .append_interaction_audit(settled(
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

/// The approval subject stays O(1) by pinning the argument value with a digest
/// instead of copying it — and the durable authority proves that pin is
/// truthful rather than decorative.
///
/// The committed digest is asserted to be exactly the digest of the arguments
/// the canonical `ToolCall` holds by value, computed independently from the
/// Ledger, and a subject whose digest names any other argument value is
/// refused by the store.
#[test]
fn the_approval_subject_pins_the_exact_canonical_arguments() {
    let store = policy_boundary_store();
    let id = interaction_id();
    store
        .append_interaction_audit(requested(&id, approval_subject()))
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
    assert_eq!(subject_call, call_id());

    // The canonical Ledger owns the exact argument value, so the digest is
    // recomputable from durable state alone. This is the property that makes
    // the bounded subject a real pin rather than an opaque 64-character field.
    let arguments = canonical_call_arguments(&store);
    assert_eq!(arguments, canonical_arguments());
    assert_eq!(
        arguments_digest,
        interaction_arguments_digest(&arguments),
        "the committed digest is the digest of the canonical ToolCall arguments"
    );

    // A digest naming any other argument value is a semantically false audit
    // record, and the durable authority refuses it outright.
    let second = InteractionId::for_attempt(&attempt(), 2);
    let InteractionSubject::Approval {
        call_id: c,
        tool_id: t,
        tool_name: n,
        reason: r,
        ..
    } = approval_subject()
    else {
        unreachable!()
    };
    assert!(
        matches!(
            store.append_interaction_audit(requested(
                &second,
                InteractionSubject::Approval {
                    call_id: c,
                    tool_id: t,
                    tool_name: n,
                    arguments_digest: interaction_arguments_digest(
                        &serde_json::json!({"path": "other.md", "limit": 20}),
                    ),
                    reason: r,
                },
            )),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "a digest that pins arguments the canonical ToolCall never carried is refused"
    );
}

/// The exact argument value the canonical Assistant `ToolCall` holds by value.
fn canonical_call_arguments(store: &SqliteConversationStore) -> serde_json::Value {
    store
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
        .expect("the canonical Assistant proposed the approved call")
}

/// An Approval audit subject must describe the canonical `ToolCall` it names.
///
/// Before this contract the store accepted a structurally valid but
/// semantically false record: an approval naming a call that was never
/// proposed, or naming the right call with the wrong tool identity. Both are
/// now refused by the durable authority, not by the coordinator.
#[test]
fn an_approval_subject_must_match_the_canonical_tool_call_it_references() {
    let store = policy_boundary_store();
    let digest = interaction_arguments_digest(&canonical_arguments());

    let missing = InteractionSubject::Approval {
        call_id: ToolCallId::new("call-that-was-never-proposed"),
        tool_id: ToolId::new("tool-alpha"),
        tool_name: "alpha".to_owned(),
        arguments_digest: digest.clone(),
        reason: "the policy asked".to_owned(),
    };
    assert!(
        matches!(
            store.append_interaction_audit(requested(
                &InteractionId::for_attempt(&attempt(), 1),
                missing
            )),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "an approval for a call no canonical Assistant proposed is refused"
    );

    let wrong_tool_id = InteractionSubject::Approval {
        call_id: call_id(),
        tool_id: ToolId::new("tool-beta"),
        tool_name: "alpha".to_owned(),
        arguments_digest: digest.clone(),
        reason: "the policy asked".to_owned(),
    };
    assert!(
        matches!(
            store.append_interaction_audit(requested(
                &InteractionId::for_attempt(&attempt(), 2),
                wrong_tool_id
            )),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "an approval naming a tool id the canonical ToolCall did not freeze is refused"
    );

    let wrong_tool_name = InteractionSubject::Approval {
        call_id: call_id(),
        tool_id: ToolId::new("tool-alpha"),
        tool_name: "beta".to_owned(),
        arguments_digest: digest,
        reason: "the policy asked".to_owned(),
    };
    assert!(
        matches!(
            store.append_interaction_audit(requested(
                &InteractionId::for_attempt(&attempt(), 3),
                wrong_tool_name
            )),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "an approval naming a tool name the canonical ToolCall did not freeze is refused"
    );

    // The truthful subject is the one the store accepts.
    store
        .append_interaction_audit(requested(
            &InteractionId::for_attempt(&attempt(), 4),
            approval_subject(),
        ))
        .expect("a subject that exactly matches the canonical ToolCall commits");
}

// ---------------------------------------------------------------------------
// The Approval subject is bound to one durable generation, not to a call id
// ---------------------------------------------------------------------------

/// Every durable fact a rejected append must leave untouched.
///
/// Comparing the whole fingerprint before and after is what turns "the store
/// returned an error" into "the store wrote nothing": the Event Journal, the
/// canonical Ledger, and the Surface head revision are the three durable
/// authorities an interaction append could otherwise disturb.
#[derive(Debug, PartialEq)]
struct DurableFingerprint {
    events: Vec<RuntimeEvent>,
    canonical: Vec<MessageBlock>,
    surface_revision: rustx::conversation::SurfaceRevision,
}

fn fingerprint(store: &SqliteConversationStore) -> DurableFingerprint {
    DurableFingerprint {
        events: journal(store),
        canonical: store.load_canonical().expect("canonical"),
        surface_revision: store.load_head().expect("head").revision,
    }
}

/// An Approval audit subject must belong to the generation its envelope names,
/// even when a canonical `ToolCall` with exactly the same identity and
/// arguments is still active on the Surface.
///
/// Turn 1 proposes `call-approved`. Turn 2 of the same attempt then asks for an
/// approval whose subject copies that call perfectly — same call id, tool id,
/// name, and canonical argument digest — and pins both audit facts to turn 2.
/// Accepting it would permanently record "turn 2 approved turn 1's `ToolCall`".
/// The store refuses it, because the canonical publication owner of
/// `call-approved` belongs to turn 1.
#[test]
fn an_approval_cannot_reference_a_canonical_tool_call_from_another_turn() {
    let store = policy_boundary_store();
    let first = Generation::first();
    let second = Generation::next("attempt-1", "2", "call-second-turn");
    commit_generation(&store, &second);

    let before = fingerprint(&store);
    let id = InteractionId::for_attempt(&second.attempt, 1);
    let error = store
        .append_interaction_audit(second.requested(&id, first.approval_subject()))
        .expect_err("turn 2 cannot approve turn 1's canonical ToolCall");
    assert!(
        matches!(error, ConversationStoreError::InvalidReference(_)),
        "a cross-turn approval is a typed reference violation, got {error:?}"
    );
    assert_eq!(
        fingerprint(&store),
        before,
        "the refused append wrote no event, no canonical message, and no Surface revision"
    );

    // The identity was not consumed and no `interaction:{id}` lifecycle row
    // exists, so the same interaction still commits once its subject names the
    // call its own generation actually proposed.
    store
        .append_interaction_audit(second.requested(&id, second.approval_subject()))
        .expect("the same identity commits for its own generation's call");
    store
        .append_interaction_audit(second.settled(&id, InteractionSettlement::Approved))
        .expect("and settles exactly once");
}

/// The same rule across attempts: attempt 1's canonical `ToolCall` is still
/// active on the Surface, and attempt 2 may not approve it.
///
/// This is the case a bare conversation-global `call_id` lookup gets wrong,
/// because the call really is findable in canonical history — it simply is not
/// owned by the generation that is asking.
#[test]
fn an_approval_cannot_reference_a_canonical_tool_call_from_another_attempt() {
    let store = policy_boundary_store();
    let first = Generation::first();
    let second = Generation::next("attempt-2", "1", "call-second-attempt");
    start_attempt(&store, &second.attempt);
    commit_generation(&store, &second);

    // Attempt 1's call is unambiguously still canonical and active.
    assert!(
        store
            .load_canonical()
            .expect("canonical")
            .iter()
            .any(|message| matches!(
                message,
                MessageBlock::Assistant(assistant)
                    if assistant.content.iter().any(|block| matches!(
                        block,
                        AssistantContentBlock::ToolCall(call) if call.id == first.call.id
                    ))
            )),
        "the cross-attempt call really is present in active canonical history"
    );

    let before = fingerprint(&store);
    let id = InteractionId::for_attempt(&second.attempt, 1);
    let error = store
        .append_interaction_audit(second.requested(&id, first.approval_subject()))
        .expect_err("attempt 2 cannot approve attempt 1's canonical ToolCall");
    assert!(
        matches!(error, ConversationStoreError::InvalidReference(_)),
        "a cross-attempt approval is a typed reference violation, got {error:?}"
    );
    assert_eq!(
        fingerprint(&store),
        before,
        "every durable authority is unchanged by the refusal"
    );
}

/// The exact generation commits: when the canonical `ToolCall`'s owning
/// attempt and turn are the ones the audit envelope names, the requested and
/// settled facts commit normally and the subject still pins the call id, tool
/// id, name, and canonical argument digest exactly.
#[test]
fn an_approval_commits_in_the_exact_generation_that_proposed_its_call() {
    let store = policy_boundary_store();
    let second = Generation::next("attempt-1", "2", "call-second-turn");
    commit_generation(&store, &second);

    for generation in [Generation::first(), second] {
        let id = InteractionId::for_attempt(
            &generation.attempt,
            generation.turn.as_str().parse().expect("numeric turn"),
        );
        store
            .append_interaction_audit(generation.requested(&id, generation.approval_subject()))
            .expect("the owning generation approves its own call");
        store
            .append_interaction_audit(generation.settled(&id, InteractionSettlement::Approved))
            .expect("and settles exactly once");

        let InteractionSubject::Approval {
            call_id: subject_call,
            tool_id: subject_tool,
            tool_name: subject_name,
            arguments_digest,
            ..
        } = generation.approval_subject()
        else {
            panic!("the approval subject is an approval");
        };
        assert_eq!(subject_call, generation.call.id);
        assert_eq!(subject_tool, generation.call.tool_id);
        assert_eq!(subject_name, generation.call.name);
        assert_eq!(
            arguments_digest,
            interaction_arguments_digest(&generation.call.arguments)
        );
    }

    assert!(
        !has_tool_start(&store),
        "an approval audit is never an execution authorization"
    );
}

/// Interaction audit payload bounds are durable-store invariants, not
/// coordinator conventions. Each of these payloads is a well-typed value that
/// deserializes cleanly and that the live coordinator would never build; the
/// store refuses every one of them.
#[test]
fn interaction_audit_payload_bounds_are_durable_invariants() {
    let store = policy_boundary_store();
    let mut ordinal = 0;
    let mut refused = |subject: InteractionSubject, what: &str| {
        ordinal += 1;
        let id = InteractionId::for_attempt(&attempt(), ordinal);
        assert!(
            matches!(
                store.append_interaction_audit(requested(&id, subject)),
                Err(ConversationStoreError::InvalidReference(_))
            ),
            "the durable authority must refuse {what}"
        );
    };

    let mut oversized_question = questionnaire_specification();
    oversized_question.questions[0].question = "p".repeat(MAX_QUESTION_TEXT_CHARS + 1);
    refused(
        InteractionSubject::Questionnaire {
            questionnaire: oversized_question,
        },
        "oversized question text",
    );
    let mut oversized_count = questionnaire_specification();
    oversized_count.questions = (0..=MAX_QUESTIONNAIRE_QUESTIONS)
        .map(|index| QuestionSpecification {
            question: format!("Question {index}"),
            header: format!("Q{index}"),
            options: vec![
                OptionSpecification {
                    label: "A".to_owned(),
                    description: "A".to_owned(),
                    preview: None,
                },
                OptionSpecification {
                    label: "B".to_owned(),
                    description: "B".to_owned(),
                    preview: None,
                },
            ],
            multi_select: false,
        })
        .collect();
    refused(
        InteractionSubject::Questionnaire {
            questionnaire: oversized_count,
        },
        "oversized question count",
    );
    let mut oversized_label = questionnaire_specification();
    oversized_label.questions[0].options[0].label = "c".repeat(MAX_OPTION_LABEL_CHARS + 1);
    refused(
        InteractionSubject::Questionnaire {
            questionnaire: oversized_label,
        },
        "oversized option label",
    );
    let mut duplicate_labels = questionnaire_specification();
    duplicate_labels.questions[0].options[1].label = "staging".to_owned();
    refused(
        InteractionSubject::Questionnaire {
            questionnaire: duplicate_labels,
        },
        "duplicate option labels",
    );
    let mut too_few_options = questionnaire_specification();
    too_few_options.questions[0].options.clear();
    refused(
        InteractionSubject::Questionnaire {
            questionnaire: too_few_options,
        },
        "an empty authored option list",
    );
    let mut reserved_label = questionnaire_specification();
    reserved_label.questions[0].options[0].label = "Type something.".to_owned();
    refused(
        InteractionSubject::Questionnaire {
            questionnaire: reserved_label,
        },
        "a client-reserved option label",
    );
    refused(
        InteractionSubject::Approval {
            call_id: call_id(),
            tool_id: ToolId::new("tool-alpha"),
            tool_name: "alpha".to_owned(),
            arguments_digest: "not-a-sha-256-digest".to_owned(),
            reason: "the policy asked".to_owned(),
        },
        "a malformed arguments digest",
    );
    refused(
        InteractionSubject::Approval {
            call_id: call_id(),
            tool_id: ToolId::new("tool-alpha"),
            tool_name: "alpha".to_owned(),
            arguments_digest: interaction_arguments_digest(&canonical_arguments()).to_uppercase(),
            reason: "the policy asked".to_owned(),
        },
        "an uppercase-hex digest, which is not the canonical representation",
    );
    refused(
        InteractionSubject::Approval {
            call_id: call_id(),
            tool_id: ToolId::new("tool-alpha"),
            tool_name: "alpha".to_owned(),
            arguments_digest: interaction_arguments_digest(&canonical_arguments()),
            reason: "r".repeat(MAX_APPROVAL_REQUEST_REASON_CHARS + 1),
        },
        "an oversized approval request reason",
    );
}

/// A questionnaire settlement must satisfy the exact requested facts and retain
/// the canonical answer decisions by value.
#[test]
fn a_questionnaire_settlement_must_satisfy_the_exact_requested_facts() {
    let store = policy_boundary_store();

    let choices_only = InteractionId::for_attempt(&attempt(), 1);
    store
        .append_interaction_audit(requested(&choices_only, questionnaire_subject()))
        .expect("requested");
    assert!(
        matches!(
            store.append_interaction_audit(settled(&choices_only, submitted_option("canary"),)),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "an option the requested questionnaire never offered is refused"
    );
    assert!(
        store
            .append_interaction_audit(settled(
                &choices_only,
                InteractionSettlement::QuestionnaireSubmitted {
                    submission: QuestionnaireSubmission {
                        answers: vec![QuestionnaireAnswerEntry {
                            question_index: 0,
                            answer: QuestionnaireAnswer::Custom(CustomAnswer {
                                answer: "canary".to_owned(),
                            }),
                        }],
                    },
                },
            ))
            .is_ok(),
        "custom answers are always available at runtime"
    );

    let multi = InteractionId::for_attempt(&attempt(), 2);
    let mut multi_spec = questionnaire_specification();
    multi_spec.questions[0].multi_select = true;
    let multi_subject = InteractionSubject::Questionnaire {
        questionnaire: multi_spec,
    };
    store
        .append_interaction_audit(requested(&multi, multi_subject))
        .expect("requested");
    assert!(
        matches!(
            store.append_interaction_audit(settled(
                &multi,
                InteractionSettlement::QuestionnaireSubmitted {
                    submission: QuestionnaireSubmission {
                        answers: vec![QuestionnaireAnswerEntry {
                            question_index: 0,
                            answer: QuestionnaireAnswer::MultipleOption(MultipleOptionAnswer {
                                selected: vec!["staging".to_owned(), "staging".to_owned()],
                            }),
                        }],
                    },
                },
            )),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "duplicated multi-selection is refused"
    );

    let declined = InteractionId::for_attempt(&attempt(), 3);
    store
        .append_interaction_audit(requested(&declined, questionnaire_subject()))
        .expect("requested");
    store
        .append_interaction_audit(settled(
            &declined,
            InteractionSettlement::QuestionnaireDeclined,
        ))
        .expect("decline is a distinct terminal settlement");
}

/// Both facts of one interaction belong to the exact same conversation +
/// attempt + turn envelope.
///
/// Before this contract the store checked only the attempt, so a settlement
/// committed under a different turn of the same attempt was durably accepted.
/// The durable authority now pins the turn too, rather than relying on the
/// coordinator happening to rebuild the same turn identity.
#[test]
fn a_settlement_is_pinned_to_the_exact_attempt_and_turn_of_its_request() {
    let store = policy_boundary_store();

    let id = InteractionId::for_attempt(&attempt(), 1);
    store
        .append_interaction_audit(requested(&id, approval_subject()))
        .expect("requested under attempt-1 / turn 1");
    assert!(
        matches!(
            store.append_interaction_audit(RuntimeEventEnvelope {
                turn_id: Some(TurnId::new("2")),
                ..settled(&id, InteractionSettlement::Approved)
            }),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "a settlement under a later turn of the same attempt is not the pinned pair"
    );
    assert!(
        matches!(
            store.append_interaction_audit(RuntimeEventEnvelope {
                attempt_id: Some(AttemptId::new("attempt-2")),
                ..settled(&id, InteractionSettlement::Approved)
            }),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "a settlement under a foreign attempt is not the pinned pair"
    );
    assert!(
        matches!(
            store.append_interaction_audit(RuntimeEventEnvelope {
                turn_id: None,
                ..settled(&id, InteractionSettlement::Approved)
            }),
            Err(ConversationStoreError::InvalidReference(_))
        ),
        "an audit fact with no turn cannot be pinned to its pair at all"
    );

    // The exact same attempt and turn is the one settlement that commits.
    store
        .append_interaction_audit(settled(&id, InteractionSettlement::Approved))
        .expect("the settlement pinned to the requested envelope commits");
}

/// A denial settlement retains the exact client-facing reason, and a Questionnaire
/// settlement retains the exact user answer, so the audit answers "what did
/// the human actually decide" without consulting current policy.
#[test]
fn settlements_retain_the_exact_decision_by_value() {
    let store = policy_boundary_store();

    let denied = InteractionId::for_attempt(&attempt(), 1);
    store
        .append_interaction_audit(requested(&denied, approval_subject()))
        .expect("requested");
    store
        .append_interaction_audit(settled(
            &denied,
            InteractionSettlement::Denied {
                reason: "the operator refused".to_owned(),
            },
        ))
        .expect("denied");

    let answered = InteractionId::for_attempt(&attempt(), 2);
    store
        .append_interaction_audit(requested(&answered, questionnaire_subject()))
        .expect("requested");
    store
        .append_interaction_audit(settled(&answered, submitted_option("staging")))
        .expect("questionnaire submitted");

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
            settlement: InteractionSettlement::QuestionnaireSubmitted { submission },
            ..
        } if matches!(
            &submission.answers[0].answer,
            QuestionnaireAnswer::SingleOption(SingleOptionAnswer { label })
                if label == "staging"
        )
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
