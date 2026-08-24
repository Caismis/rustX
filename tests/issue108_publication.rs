//! Issue #108 (FND-03) — the durable publication plane's store contract.
//!
//! Every regression here builds an **exact committed prefix** of the three
//! Issue #108 linearization points in a file-backed `SQLite` conversation,
//! drops the store (the "process died here" boundary), reopens the same
//! database, and asserts what the durable authority permits and what recovery
//! classifies:
//!
//! ```text
//! P — ModelRequestCompleted durable        (Event Journal)
//! U — final frame + terminal marker        (publication plane, one transaction)
//! C — canonical Assistant durable          (Message Ledger)
//!
//! required ordering            P < U < C
//! durable-store implication    C => U => P
//! ```
//!
//! The crash boundary is a `drop` and the reopen is a
//! `SqliteConversationStore::open`. There is no sleep, no timer, and no
//! timing assumption anywhere.
//!
//! The Agent-Loop-facing half of the same contract — bounded coalescing, the
//! fake-clock latency flush, release-after-commit, and the settlement each
//! control-flow exit reaches — lives in the in-crate scripted suite
//! `tests/scripted/issue108_publication.rs`, because it needs the scripted
//! model adapter.

use chrono::{DateTime, TimeZone, Utc};
use rustx::context::ContextGeneration;
use rustx::durable::{ConversationStore, ConversationStoreError, SqliteConversationStore};
use rustx::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
};
use rustx::model::catalog::{ModelCapabilities, ModelCompat};
use rustx::model::{
    ModelFinishReason, ModelInvocationConfig, ModelProtocol, RequestIdentity, RequestParams,
    RequestSnapshot,
};
use rustx::publication::{
    PublicationAuditBlock, PublicationAuditKind, PublicationFrame, PublicationPayload,
    PublicationStreamStart,
};
use rustx::runtime::identity::{
    AttemptId, CapabilityRevision, ConversationId, EventId, MessageId, PublicationStreamId,
    RequestId, ToolCallId, ToolId, TurnId,
};
use rustx::runtime::recovery::{RecoveryReport, recover};
use rustx::runtime::types::RuntimeClock;
use rustx::tools::types::{ToolCall, ToolCallStart};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const CONVERSATION: &str = "conv-fnd03";

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

fn attempt() -> AttemptId {
    AttemptId::new("attempt-1")
}

/// Commits the one durable request-start transaction of a turn and returns
/// the started request identity.
fn start_request(store: &SqliteConversationStore, turn: &str) -> RequestId {
    let head = store.load_head().expect("head");
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new(turn),
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

fn envelope(event_id: &str, turn: &str, event: RuntimeEvent) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 0,
        conversation_id: conversation_id(),
        attempt_id: Some(attempt()),
        turn_id: Some(TurnId::new(turn)),
        timestamp: fixed_time(),
        event,
    }
}

/// Commits **P** for one exact request.
fn commit_provider_outcome(store: &SqliteConversationStore, turn: &str, request_id: &RequestId) {
    store
        .append_event(envelope(
            &format!("request-completed-{turn}"),
            turn,
            RuntimeEvent::ModelRequestCompleted {
                request_id: request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        ))
        .expect("provider outcome");
}

fn stream_start(request_id: &RequestId, turn: &str, message_id: &str) -> PublicationStreamStart {
    let message_id = MessageId::new(message_id);
    PublicationStreamStart {
        stream_id: PublicationStreamId::for_request(&attempt(), &message_id),
        attempt_id: attempt(),
        turn_id: TurnId::new(turn),
        request_id: request_id.clone(),
        message_id,
    }
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

fn text(sequence: u64, start: &PublicationStreamStart, suffix: &str) -> PublicationFrame {
    frame(
        start,
        sequence,
        PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: suffix.to_owned(),
        },
    )
}

fn assistant(message_id: &MessageId, body: &str) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: message_id.clone(),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: body.to_owned(),
        })],
    })
}

fn committed_event(message_id: &MessageId, turn: &str) -> RuntimeEventEnvelope {
    envelope(
        &format!("assistant-committed-{turn}"),
        turn,
        RuntimeEvent::AssistantMessageCommitted {
            message_id: message_id.clone(),
        },
    )
}

/// Runs the real recovery pipeline over a freshly reopened durable store.
fn recover_reopened(durable: &Durable) -> RecoveryReport {
    let store = durable.open();
    recover(&store, &FixedClock).expect("recovery succeeds")
}

// ---------------------------------------------------------------------------
// P / U / C ordering (regressions 4, 5, 6)
// ---------------------------------------------------------------------------

/// **Regression 4.** U may never precede P: the durable store rejects a
/// publication terminal for a request whose provider outcome is not yet
/// durable, and accepts the identical transaction once P exists.
#[test]
fn publication_terminal_requires_a_durable_provider_outcome() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    store.open_publication_stream(&start).expect("open");
    store
        .stage_publication_frames(&[text(0, &start, "hello")])
        .expect("stage");

    let terminal = [text(1, &start, " world")];
    let rejected = store.commit_publication_terminal(&start.stream_id, &terminal);
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("no durable provider outcome")
        ),
        "U without P must be rejected, got {rejected:?}"
    );

    commit_provider_outcome(&store, "1", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &terminal)
        .expect("U commits once P is durable");
}

/// **Regression 5.** The final publication frame and the publication terminal
/// marker are one transaction, including the terminal-only case: after the
/// commit the stream is publication-complete *and* carries its final frame,
/// and there is no intermediate state where one exists without the other.
#[test]
fn publication_terminal_frame_and_marker_commit_together() {
    let durable = Durable::new();
    // Case 1: a terminal transaction that still carries visible payload.
    {
        let store = durable.open();
        store.initialize(&[]).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "msg-1");
        store.open_publication_stream(&start).expect("open");
        commit_provider_outcome(&store, "1", &request_id);
        store
            .commit_publication_terminal(&start.stream_id, &[text(0, &start, "tail")])
            .expect("U");
        // A second terminal transaction is rejected: U happens exactly once.
        assert!(matches!(
            store.commit_publication_terminal(&start.stream_id, &[text(1, &start, "more")]),
            Err(ConversationStoreError::PublicationViolation(_))
        ));
        // A publication-complete stream accepts no further staging either.
        assert!(matches!(
            store.stage_publication_frames(&[text(1, &start, "late")]),
            Err(ConversationStoreError::PublicationViolation(_))
        ));
    }
    // The marker survives the crash boundary together with its frame.
    let store = durable.open();
    let unsettled = store
        .load_unsettled_publication_streams()
        .expect("unsettled streams");
    assert_eq!(unsettled.len(), 1);
    assert!(
        unsettled[0].reached_publication_terminal(),
        "U is durable after the crash boundary"
    );
    assert_eq!(unsettled[0].audit_kind(), PublicationAuditKind::Unaccepted);

    // Case 2: provider completion with no buffered visible payload still
    // commits a terminal-only frame together with its marker.
    let empty = Durable::new();
    let store = empty.open();
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    store.open_publication_stream(&start).expect("open");
    commit_provider_outcome(&store, "1", &request_id);
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[frame(&start, 0, PublicationPayload::TerminalOnly)],
        )
        .expect("terminal-only U");
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("audit");
    assert_eq!(audit.kind, PublicationAuditKind::Unaccepted);
    assert!(
        audit.content.is_empty(),
        "a terminal-only publication released no visible payload"
    );
}

/// **Regression 6.** C may never precede U: the durable store rejects
/// canonical Assistant acceptance of a published stream that has not reached
/// its publication terminal, and no Ledger row is written by the rejection.
#[test]
fn canonical_acceptance_requires_the_publication_terminal() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    store.open_publication_stream(&start).expect("open");
    store
        .stage_publication_frames(&[text(0, &start, "hello")])
        .expect("stage");
    commit_provider_outcome(&store, "1", &request_id);

    let message = assistant(&start.message_id, "hello");
    let rejected = store.commit_canonical_publication(
        &start.stream_id,
        &message,
        committed_event(&start.message_id, "1"),
    );
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("no durable publication terminal")
        ),
        "C without U must be rejected, got {rejected:?}"
    );
    assert!(
        store.load_canonical().expect("canonical").is_empty(),
        "the rejected acceptance wrote no Ledger row"
    );

    store
        .commit_publication_terminal(&start.stream_id, &[text(1, &start, "!")])
        .expect("U");
    store
        .commit_canonical_publication(
            &start.stream_id,
            &message,
            committed_event(&start.message_id, "1"),
        )
        .expect("C commits once U is durable");
    assert_eq!(store.load_canonical().expect("canonical").len(), 1);
}

// ---------------------------------------------------------------------------
// Settlement exclusivity and staging lifecycle (regressions 7, 14, 15)
// ---------------------------------------------------------------------------

/// **Regression 7.** The three settlements are mutually exclusive. Once a
/// stream settles — canonically or as either audit — every other settlement
/// of that same stream is permanently forbidden.
#[test]
fn the_three_settlements_are_mutually_exclusive() {
    // Canonical first: an audit afterwards is forbidden.
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    store.open_publication_stream(&start).expect("open");
    commit_provider_outcome(&store, "1", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(0, &start, "hi")])
        .expect("U");
    store
        .commit_canonical_publication(
            &start.stream_id,
            &assistant(&start.message_id, "hi"),
            committed_event(&start.message_id, "1"),
        )
        .expect("C");
    assert!(matches!(
        store.terminalize_publication_audit(&start.stream_id, fixed_time()),
        Err(ConversationStoreError::PublicationViolation(_))
    ));

    // Audit first: canonical acceptance afterwards is forbidden.
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    store.open_publication_stream(&start).expect("open");
    commit_provider_outcome(&store, "1", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(0, &start, "hi")])
        .expect("U");
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("audit");
    assert_eq!(audit.kind, PublicationAuditKind::Unaccepted);
    let rejected = store.commit_canonical_publication(
        &start.stream_id,
        &assistant(&start.message_id, "hi"),
        committed_event(&start.message_id, "1"),
    );
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("permanently forbidden")
        ),
        "canonical acceptance after an audit must be forbidden, got {rejected:?}"
    );
    assert!(store.load_canonical().expect("canonical").is_empty());
    // A second audit is equally forbidden: a stream settles exactly once.
    assert!(matches!(
        store.terminalize_publication_audit(&start.stream_id, fixed_time()),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
}

/// **Regression 14.** The canonical transition clears the stream's
/// publication staging atomically, and no publication audit is created for a
/// canonically accepted stream — before or after a restart.
#[test]
fn canonical_acceptance_clears_staging_and_creates_no_audit() {
    let durable = Durable::new();
    let stream_id;
    {
        let store = durable.open();
        store.initialize(&[]).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "msg-1");
        stream_id = start.stream_id.clone();
        store.open_publication_stream(&start).expect("open");
        for (sequence, chunk) in ["a", "b", "c"].into_iter().enumerate() {
            store
                .stage_publication_frames(&[text(sequence as u64, &start, chunk)])
                .expect("stage");
        }
        commit_provider_outcome(&store, "1", &request_id);
        store
            .commit_publication_terminal(&start.stream_id, &[text(3, &start, "d")])
            .expect("U");
        store
            .commit_canonical_publication(
                &start.stream_id,
                &assistant(&start.message_id, "abcd"),
                committed_event(&start.message_id, "1"),
            )
            .expect("C");
        assert!(
            store
                .load_unsettled_publication_streams()
                .expect("unsettled")
                .is_empty(),
            "the canonical transition settled the stream"
        );
        assert!(
            store
                .load_publication_audit(&start.stream_id)
                .expect("audit read")
                .is_none(),
            "a canonically accepted stream has no audit"
        );
    }

    // Recovery over the reopened database creates nothing: the Ledger is the
    // authority and no staging survived.
    let report = recover_reopened(&durable);
    assert!(
        report.publication_classes().is_empty(),
        "a settled stream is never reclassified"
    );
    assert!(report.reconciliation().publication_audits.is_empty());
    let store = durable.open();
    assert!(
        store
            .load_publication_audit(&stream_id)
            .expect("audit read")
            .is_none()
    );
    assert_eq!(store.load_canonical().expect("canonical").len(), 1);
}

/// **Regression 15.** Recovery consolidation leaves exactly one bounded
/// immutable audit object rather than permanent per-frame staging rows, and
/// the consolidated content is the released output, not the frame count.
#[test]
fn recovery_consolidates_many_frames_into_one_bounded_audit() {
    let durable = Durable::new();
    let stream_id;
    {
        let store = durable.open();
        store.initialize(&[]).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "msg-1");
        stream_id = start.stream_id.clone();
        store.open_publication_stream(&start).expect("open");
        for sequence in 0..500u64 {
            store
                .stage_publication_frames(&[text(sequence, &start, "x")])
                .expect("stage");
        }
        commit_provider_outcome(&store, "1", &request_id);
        store
            .commit_publication_terminal(&start.stream_id, &[text(500, &start, "!")])
            .expect("U");
        // CRASH: U is durable, the canonical Assistant never committed.
    }

    let report = recover_reopened(&durable);
    assert_eq!(report.publication_classes().len(), 1);
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Unaccepted
    );
    assert_eq!(
        report.reconciliation().publication_audits,
        vec![(stream_id.clone(), PublicationAuditKind::Unaccepted)]
    );

    let store = durable.open();
    let audit = store
        .load_publication_audit(&stream_id)
        .expect("audit read")
        .expect("the settled stream has one audit");
    assert_eq!(
        audit.content.len(),
        1,
        "501 frames consolidated into one bounded audit block"
    );
    let PublicationAuditBlock::Text { text, .. } = &audit.content[0] else {
        panic!("the released output is text");
    };
    assert_eq!(text.len(), 501, "the released bytes are preserved exactly");
    assert!(
        store
            .load_unsettled_publication_streams()
            .expect("unsettled")
            .is_empty(),
        "no staging survives the audit terminalization"
    );

    // A second restart after a successful recovery changes nothing: durable
    // state stops changing (the recovery-prefix invariant).
    let second = recover_reopened(&durable);
    assert!(second.publication_classes().is_empty());
    assert!(second.reconciliation().publication_audits.is_empty());
}

// ---------------------------------------------------------------------------
// Crash-boundary classification (regressions 8, 10, 11, 13, 19)
// ---------------------------------------------------------------------------

/// **Regressions 8 and 10.** Publication that never reached its own durable
/// terminal is Incomplete, whether or not the provider outcome is durably
/// known. The definition is on the publication boundary, never the provider
/// boundary.
#[test]
fn a_stream_without_its_publication_terminal_is_incomplete() {
    for provider_outcome_is_durable in [false, true] {
        let durable = Durable::new();
        let stream_id;
        {
            let store = durable.open();
            store.initialize(&[]).expect("initialize");
            let request_id = start_request(&store, "1");
            let start = stream_start(&request_id, "1", "msg-1");
            stream_id = start.stream_id.clone();
            store.open_publication_stream(&start).expect("open");
            store
                .stage_publication_frames(&[text(0, &start, "released")])
                .expect("stage");
            if provider_outcome_is_durable {
                commit_provider_outcome(&store, "1", &request_id);
            }
            // CRASH: staged output exists, U never committed.
        }

        let report = recover_reopened(&durable);
        assert_eq!(report.publication_classes().len(), 1);
        assert_eq!(
            report.publication_classes()[0].kind,
            PublicationAuditKind::Incomplete,
            "P present = {provider_outcome_is_durable}: publication still has no terminal"
        );
        let store = durable.open();
        let audit = store
            .load_publication_audit(&stream_id)
            .expect("audit read")
            .expect("audit exists");
        assert_eq!(audit.kind, PublicationAuditKind::Incomplete);
        assert!(matches!(
            &audit.content[0],
            PublicationAuditBlock::Text { text, .. } if text == "released"
        ));
    }
}

/// **Regression 11.** U committed and C never did: the settlement is
/// Unaccepted, the released output was complete, and no Assistant message is
/// canonical.
#[test]
fn a_published_stream_without_canonical_acceptance_is_unaccepted() {
    let durable = Durable::new();
    let stream_id;
    {
        let store = durable.open();
        store.initialize(&[]).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "msg-1");
        stream_id = start.stream_id.clone();
        store.open_publication_stream(&start).expect("open");
        store
            .stage_publication_frames(&[text(0, &start, "complete ")])
            .expect("stage");
        commit_provider_outcome(&store, "1", &request_id);
        store
            .commit_publication_terminal(&start.stream_id, &[text(1, &start, "answer")])
            .expect("U");
        // CRASH: after U, before the canonical Assistant commit.
    }

    let report = recover_reopened(&durable);
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Unaccepted
    );
    let store = durable.open();
    let audit = store
        .load_publication_audit(&stream_id)
        .expect("audit read")
        .expect("audit exists");
    assert!(matches!(
        &audit.content[0],
        PublicationAuditBlock::Text { text, .. } if text == "complete answer"
    ));
    assert!(
        store.load_canonical().expect("canonical").is_empty(),
        "an Unaccepted publication never becomes conversation history"
    );
}

/// **Regression 13.** A partially released tool-call proposal plus process
/// death settles Incomplete, and the proposal can never acquire a dependent
/// Tool Plane execution fact.
#[test]
fn a_partial_tool_proposal_settles_incomplete_and_can_never_execute() {
    let durable = Durable::new();
    let call_id = ToolCallId::new("call-1");
    let stream_id;
    {
        let store = durable.open();
        store.initialize(&[]).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "msg-1");
        stream_id = start.stream_id.clone();
        store.open_publication_stream(&start).expect("open");
        store
            .stage_publication_frames(&[frame(
                &start,
                0,
                PublicationPayload::ProposedToolCallStarted {
                    block_index: ContentBlockIndex::new(0),
                    call: ToolCallStart {
                        id: call_id.clone(),
                        tool_id: ToolId::new("tool-alpha"),
                        name: "alpha".to_owned(),
                    },
                },
            )])
            .expect("stage proposal start");
        store
            .stage_publication_frames(&[frame(
                &start,
                1,
                PublicationPayload::ProposedToolCallArgumentsSuffix {
                    block_index: ContentBlockIndex::new(0),
                    call_id: call_id.clone(),
                    suffix: r#"{"path":"#.to_owned(),
                },
            )])
            .expect("stage partial arguments");
        // CRASH: the proposal never finished assembling.
    }

    let report = recover_reopened(&durable);
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Incomplete
    );
    let store = durable.open();
    let audit = store
        .load_publication_audit(&stream_id)
        .expect("audit read")
        .expect("audit exists");
    let PublicationAuditBlock::ProposedToolCall {
        arguments,
        complete,
        ..
    } = &audit.content[0]
    else {
        panic!("the audit records the model proposal");
    };
    assert_eq!(arguments, r#"{"path":"#);
    assert!(!complete, "the proposal never finished assembling");
    assert_eq!(audit.proposed_call_ids(), vec![call_id.clone()]);

    // The hard invariant: no dependent Tool Plane execution fact may exist.
    let rejected = store.append_event(envelope(
        "tool-started",
        "1",
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: call_id,
            tool_id: ToolId::new("tool-alpha"),
        },
    ));
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("may never execute")
        ),
        "an audited proposal must never acquire an execution fact, got {rejected:?}"
    );
}

/// **Regression 12 (durable half).** A complete model-proposed tool call that
/// was released but never accepted settles as an Unaccepted proposal audit:
/// no canonical Assistant, and no `ToolExecutionStarted` is permitted for it.
#[test]
fn a_complete_unaccepted_proposal_never_acquires_an_execution_fact() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    let call_id = ToolCallId::new("call-1");
    store.open_publication_stream(&start).expect("open");
    store
        .stage_publication_frames(&[frame(
            &start,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call_id.clone(),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                },
            },
        )])
        .expect("stage proposal start");
    commit_provider_outcome(&store, "1", &request_id);
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[frame(
                &start,
                1,
                PublicationPayload::ProposedToolCallCompleted {
                    block_index: ContentBlockIndex::new(0),
                    call: ToolCall {
                        id: call_id.clone(),
                        tool_id: ToolId::new("tool-alpha"),
                        name: "alpha".to_owned(),
                        arguments: serde_json::json!({"path": "."}),
                    },
                },
            )],
        )
        .expect("U");
    // The preflight contract failed after a complete model output: the stream
    // terminalizes as an Unaccepted proposal audit.
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("audit");
    assert_eq!(audit.kind, PublicationAuditKind::Unaccepted);
    assert!(matches!(
        &audit.content[0],
        PublicationAuditBlock::ProposedToolCall { complete, .. } if *complete
    ));
    assert!(
        store.load_canonical().expect("canonical").is_empty(),
        "an Unaccepted publication never becomes conversation history"
    );
    assert!(matches!(
        store.append_event(envelope(
            "tool-started",
            "1",
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_id,
                tool_id: ToolId::new("tool-alpha"),
            },
        )),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
}

/// **Regression 19.** A cold reopen classifies an old stream solely from its
/// frozen request identity and the durable P/U/C evidence. A newer request
/// generation admitted after the restart neither reclassifies nor absorbs it.
#[test]
fn cold_reopen_classifies_the_old_stream_from_its_frozen_request() {
    let durable = Durable::new();
    let old_stream;
    let old_request;
    {
        let store = durable.open();
        store.initialize(&[]).expect("initialize");
        old_request = start_request(&store, "1");
        let start = stream_start(&old_request, "1", "msg-1");
        old_stream = start.stream_id.clone();
        store.open_publication_stream(&start).expect("open");
        store
            .stage_publication_frames(&[text(0, &start, "old generation")])
            .expect("stage");
        commit_provider_outcome(&store, "1", &old_request);
        // CRASH: P is durable, U never committed.
    }

    // The reopened store sees exactly the frozen historical identities.
    let store = durable.open();
    let unsettled = store
        .load_unsettled_publication_streams()
        .expect("unsettled");
    assert_eq!(unsettled.len(), 1);
    assert_eq!(unsettled[0].start.request_id, old_request);
    assert_eq!(unsettled[0].start.turn_id, TurnId::new("1"));
    assert!(!unsettled[0].reached_publication_terminal());
    assert_eq!(unsettled[0].audit_kind(), PublicationAuditKind::Incomplete);
    drop(store);

    let report = recover_reopened(&durable);
    assert_eq!(
        report.publication_classes()[0].kind,
        PublicationAuditKind::Incomplete,
        "P present, U absent: publication never reached its own terminal"
    );

    // A new request generation admitted after recovery is a separate stream
    // and leaves the settled audit untouched.
    let store = durable.open();
    let new_request = start_request(&store, "2");
    assert_ne!(new_request, old_request);
    let new_start = stream_start(&new_request, "2", "msg-2");
    store.open_publication_stream(&new_start).expect("open");
    commit_provider_outcome(&store, "2", &new_request);
    store
        .commit_publication_terminal(&new_start.stream_id, &[text(0, &new_start, "new")])
        .expect("U");
    store
        .commit_canonical_publication(
            &new_start.stream_id,
            &assistant(&new_start.message_id, "new"),
            committed_event(&new_start.message_id, "2"),
        )
        .expect("C");
    let old_audit = store
        .load_publication_audit(&old_stream)
        .expect("audit read")
        .expect("the old stream stayed audited");
    assert_eq!(old_audit.kind, PublicationAuditKind::Incomplete);
    assert_eq!(old_audit.request_id, old_request);
    assert_eq!(
        store.load_canonical().expect("canonical").len(),
        1,
        "only the new generation became conversation history"
    );
}

// ---------------------------------------------------------------------------
// Identity and generation pinning (regression 17, durable half)
// ---------------------------------------------------------------------------

/// **Regression 17 (durable half).** A publication stream is pinned to the
/// exact request generation that opened it: reopening the same identity under
/// a different request, turn, or provisional message is rejected, and frames
/// naming a foreign message identity or an out-of-order sequence are rejected.
#[test]
fn a_publication_stream_is_pinned_to_its_opening_generation() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let first = start_request(&store, "1");
    let start = stream_start(&first, "1", "msg-1");
    store.open_publication_stream(&start).expect("open");
    // Re-opening the identical frozen identity is idempotent.
    store
        .open_publication_stream(&start)
        .expect("identical reopen is idempotent");

    let second = start_request(&store, "2");
    let mut spliced = start.clone();
    spliced.request_id = second;
    let rejected = store.open_publication_stream(&spliced);
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("different request generation")
        ),
        "a newer generation must never splice into an in-flight stream, got {rejected:?}"
    );

    let mut foreign = text(0, &start, "x");
    foreign.message_id = MessageId::new("msg-other");
    assert!(matches!(
        store.stage_publication_frames(&[foreign]),
        Err(ConversationStoreError::PublicationViolation(_))
    ));

    assert!(
        matches!(
            store.stage_publication_frames(&[text(7, &start, "x")]),
            Err(ConversationStoreError::PublicationViolation(_))
        ),
        "publication frame ordering is deterministic and gapless"
    );
    store
        .stage_publication_frames(&[text(0, &start, "x")])
        .expect("the next contiguous sequence is accepted");
}

// ---------------------------------------------------------------------------
// Event Journal write amplification (regression 16)
// ---------------------------------------------------------------------------

/// **Regression 16.** The Event Journal no longer grows per Assistant text,
/// reasoning, or tool-argument increment. A stream that releases hundreds of
/// frames adds exactly the same number of Journal rows as one that releases
/// none.
#[test]
fn the_event_journal_does_not_grow_per_streamed_increment() {
    fn journal_rows(frame_count: u64) -> usize {
        let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
        store.initialize(&[]).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "msg-1");
        store.open_publication_stream(&start).expect("open");
        for sequence in 0..frame_count {
            store
                .stage_publication_frames(&[text(sequence, &start, "token ")])
                .expect("stage");
        }
        commit_provider_outcome(&store, "1", &request_id);
        store
            .commit_publication_terminal(&start.stream_id, &[text(frame_count, &start, "!")])
            .expect("U");
        store
            .commit_canonical_publication(
                &start.stream_id,
                &assistant(&start.message_id, "body"),
                committed_event(&start.message_id, "1"),
            )
            .expect("C");
        let mut rows = 0;
        let mut cursor = None;
        loop {
            let page = store.read_events(cursor, 64).expect("event page");
            if page.events.is_empty() {
                break;
            }
            rows += page.events.len();
            cursor = page.next_sequence;
        }
        rows
    }

    let quiet = journal_rows(0);
    let chatty = journal_rows(500);
    assert_eq!(
        quiet, chatty,
        "500 released increments cost zero additional Event Journal rows"
    );
    assert_eq!(
        quiet, 3,
        "one turn journals exactly ModelRequestStarted, ModelRequestCompleted, and AssistantMessageCommitted"
    );
}

/// A provider outcome may be recorded exactly once for one exact request, and
/// only for a request that actually started.
#[test]
fn a_provider_outcome_names_one_started_request_exactly_once() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");

    let unknown = store.append_event(envelope(
        "unknown-request",
        "1",
        RuntimeEvent::ModelRequestCompleted {
            request_id: RequestId::new("request-never-started"),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ));
    assert!(
        matches!(
            &unknown,
            Err(ConversationStoreError::InvalidReference(detail))
                if detail.contains("never started")
        ),
        "P must name an actually started request, got {unknown:?}"
    );

    commit_provider_outcome(&store, "1", &request_id);
    assert!(matches!(
        store.append_event(envelope(
            "duplicate-outcome",
            "1",
            RuntimeEvent::ModelRequestCompleted {
                request_id,
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            },
        )),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
}
