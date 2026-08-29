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

#![allow(clippy::too_many_lines)] // deterministic store scenarios stay linear

use chrono::{DateTime, TimeZone, Utc};
use rustx::context::ContextGeneration;
use rustx::durable::{ConversationStore, ConversationStoreError, SqliteConversationStore};
use rustx::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock, ToolMessageBlock,
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
    AgentId, AttemptId, CapabilityRevision, ConversationId, EventId, MessageId,
    PublicationStreamId, RequestId, SubagentId, ToolCallId, ToolId, TurnId,
};
use rustx::runtime::recovery::{RecoveryReport, recover};
use rustx::runtime::types::RuntimeClock;
use rustx::tools::types::{ToolCall, ToolCallStart};
use rustx::tools::types::{ToolExecutionResult, ToolExecutionStatus};
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

fn stream_start(request_id: &RequestId, turn: &str, _message_id: &str) -> PublicationStreamStart {
    // The Request Snapshot owns this mapping now; the old fixture argument is
    // retained only to keep each scenario visually tied to its turn.
    let message_id = MessageId::new(format!("{}-agent-{turn}", attempt()));
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

fn assistant_with_tool(message_id: &MessageId, call_id: &ToolCallId) -> MessageBlock {
    assistant_with_tools(
        message_id,
        vec![ToolCall {
            id: call_id.clone(),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        }],
    )
}

fn assistant_with_tools(message_id: &MessageId, calls: Vec<ToolCall>) -> MessageBlock {
    MessageBlock::Assistant(AssistantMessageBlock {
        id: message_id.clone(),
        content: calls
            .into_iter()
            .map(AssistantContentBlock::ToolCall)
            .collect(),
    })
}

fn tool_message(message_id: &str, call_id: &ToolCallId) -> MessageBlock {
    tool_message_with_tool(message_id, call_id, ToolId::new("tool-alpha"))
}

fn tool_message_with_tool(message_id: &str, call_id: &ToolCallId, tool_id: ToolId) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new(message_id),
        tool_call_id: call_id.clone(),
        tool_id,
        result: successful_tool_result(),
    })
}

fn successful_tool_result() -> ToolExecutionResult {
    ToolExecutionResult {
        status: ToolExecutionStatus::Success,
        content: Vec::new(),
        duration_ms: 1,
        exit_code: Some(0),
        artifacts: Vec::new(),
        truncation: None,
        managed_output: None,
    }
}

fn proposal_start_frame(
    start: &PublicationStreamStart,
    sequence: u64,
    call_id: &ToolCallId,
) -> PublicationFrame {
    proposal_start_frame_with(
        start,
        sequence,
        ContentBlockIndex::new(0),
        call_id,
        ToolId::new("tool-alpha"),
        "alpha",
    )
}

fn proposal_start_frame_with(
    start: &PublicationStreamStart,
    sequence: u64,
    block_index: ContentBlockIndex,
    call_id: &ToolCallId,
    tool_id: ToolId,
    name: &str,
) -> PublicationFrame {
    frame(
        start,
        sequence,
        PublicationPayload::ProposedToolCallStarted {
            block_index,
            call: ToolCallStart {
                id: call_id.clone(),
                tool_id,
                name: name.to_owned(),
            },
        },
    )
}

fn proposal_arguments_frame(
    start: &PublicationStreamStart,
    sequence: u64,
    block_index: ContentBlockIndex,
    call_id: &ToolCallId,
    suffix: &str,
) -> PublicationFrame {
    frame(
        start,
        sequence,
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index,
            call_id: call_id.clone(),
            suffix: suffix.to_owned(),
        },
    )
}

fn proposal_complete_frame_with(
    start: &PublicationStreamStart,
    sequence: u64,
    block_index: ContentBlockIndex,
    call_id: &ToolCallId,
    tool_id: ToolId,
    name: &str,
) -> PublicationFrame {
    frame(
        start,
        sequence,
        PublicationPayload::ProposedToolCallCompleted {
            block_index,
            call: ToolCall {
                id: call_id.clone(),
                tool_id,
                name: name.to_owned(),
                arguments: serde_json::json!({}),
            },
        },
    )
}

fn stage_completed_proposal(
    store: &SqliteConversationStore,
    start: &PublicationStreamStart,
    sequence: u64,
    block_index: ContentBlockIndex,
    call_id: &ToolCallId,
    tool_id: ToolId,
    name: &str,
) -> u64 {
    store
        .stage_publication_frames(&[proposal_start_frame_with(
            start,
            sequence,
            block_index,
            call_id,
            tool_id.clone(),
            name,
        )])
        .expect("proposal Started");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            start,
            sequence + 1,
            block_index,
            call_id,
            tool_id,
            name,
        )])
        .expect("proposal Completed");
    sequence + 2
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

fn opened_test_stream(
    store: &SqliteConversationStore,
    turn: &str,
) -> (RequestId, PublicationStreamStart) {
    let request_id = start_request(store, turn);
    let start = stream_start(&request_id, turn, "ignored");
    store.open_publication_stream(&start).expect("open stream");
    (request_id, start)
}

fn event_count(store: &SqliteConversationStore) -> usize {
    store.read_events(None, 256).expect("events").events.len()
}

// ---------------------------------------------------------------------------
// Proposal staging state machine (Issue #108)
// ---------------------------------------------------------------------------

/// A suffix and a completion cannot create ownership by themselves. The
/// failed transaction leaves sequence zero available for the real Started
/// frame, proving that no frame or proposal row was partially inserted.
#[test]
fn proposal_arguments_and_completion_require_a_started_owner() {
    for (label, payload) in [
        (
            "arguments",
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: ToolCallId::new("orphan-arguments"),
                suffix: "{}".to_owned(),
            },
        ),
        (
            "completion",
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCall {
                    id: ToolCallId::new("orphan-completion"),
                    tool_id: ToolId::new("tool-alpha"),
                    name: "alpha".to_owned(),
                    arguments: serde_json::json!({}),
                },
            },
        ),
    ] {
        let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
        store.initialize(&[]).expect("initialize");
        let (_request_id, start) = opened_test_stream(&store, "proposal-orphan");
        let before_head = store.load_head().expect("head");
        let before_events = event_count(&store);
        let rejected = store.stage_publication_frames(&[frame(&start, 0, payload)]);
        assert!(
            matches!(
                &rejected,
                Err(ConversationStoreError::PublicationViolation(detail))
                    if detail.contains("no Started frame")
            ),
            "orphan {label} must be rejected: {rejected:?}"
        );
        assert_eq!(store.load_head().expect("head"), before_head);
        assert_eq!(event_count(&store), before_events);
        assert_eq!(store.load_canonical().expect("canonical").len(), 0);
        assert!(
            store
                .load_publication_audit(&start.stream_id)
                .expect("audit")
                .is_none()
        );
        assert_eq!(
            store
                .load_unsettled_publication_streams()
                .expect("streams")
                .len(),
            1
        );

        // Sequence zero and the ownership slot were untouched by the
        // rejected transaction.
        let call_id = ToolCallId::new(format!("valid-{label}"));
        store
            .stage_publication_frames(&[proposal_start_frame(&start, 0, &call_id)])
            .expect("Started can be staged at the unchanged sequence");
    }
}

/// Duplicate starts, duplicate completions, and argument suffixes after
/// completion are all rejected without consuming the next frame sequence.
#[test]
fn proposal_duplicate_and_post_completion_transitions_are_rejected() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (_request_id, start) = opened_test_stream(&store, "proposal-duplicates");
    let call_id = ToolCallId::new("duplicate-call");
    store
        .stage_publication_frames(&[proposal_start_frame(&start, 0, &call_id)])
        .expect("first Started");

    let duplicate_start =
        store.stage_publication_frames(&[proposal_start_frame(&start, 1, &call_id)]);
    assert!(matches!(
        duplicate_start,
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    store
        .stage_publication_frames(&[proposal_arguments_frame(
            &start,
            1,
            ContentBlockIndex::new(0),
            &call_id,
            "{}",
        )])
        .expect("the duplicate start did not consume sequence one");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            &start,
            2,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        )])
        .expect("completion");

    let duplicate_completion = store.stage_publication_frames(&[proposal_complete_frame_with(
        &start,
        3,
        ContentBlockIndex::new(0),
        &call_id,
        ToolId::new("tool-alpha"),
        "alpha",
    )]);
    assert!(matches!(
        duplicate_completion,
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    let suffix_after_completion = store.stage_publication_frames(&[proposal_arguments_frame(
        &start,
        3,
        ContentBlockIndex::new(0),
        &call_id,
        "more",
    )]);
    assert!(matches!(
        suffix_after_completion,
        Err(ConversationStoreError::PublicationViolation(_))
    ));

    // Both rejected transitions left sequence three free, and the complete
    // proposal can still enter U and the bounded audit exactly once.
    let request_id = RequestId::new(start.request_id.as_str());
    commit_provider_outcome(&store, "proposal-duplicates", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(3, &start, "done")])
        .expect("U after rejected duplicate transitions");
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("audit")
        .0;
    assert_eq!(audit.proposed_call_ids(), vec![call_id]);
    assert!(matches!(
        audit.content[0],
        PublicationAuditBlock::ProposedToolCall { complete: true, .. }
    ));
}

/// The frozen block index, tool identity, and tool name belong to the Started
/// owner. A mismatched completion is rejected atomically, and the exact
/// completion can reuse that same sequence afterwards.
#[test]
fn proposal_completion_must_match_frozen_identity() {
    let cases = [
        (
            "foreign-block",
            ContentBlockIndex::new(1),
            ToolId::new("tool-alpha"),
            "alpha",
        ),
        (
            "foreign-tool",
            ContentBlockIndex::new(0),
            ToolId::new("tool-beta"),
            "alpha",
        ),
        (
            "foreign-name",
            ContentBlockIndex::new(0),
            ToolId::new("tool-alpha"),
            "beta",
        ),
    ];
    for (label, block_index, tool_id, name) in cases {
        let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
        store.initialize(&[]).expect("initialize");
        let (_request_id, start) = opened_test_stream(&store, &format!("proposal-{label}"));
        let call_id = ToolCallId::new(format!("call-{label}"));
        store
            .stage_publication_frames(&[proposal_start_frame(&start, 0, &call_id)])
            .expect("Started");
        let rejected = store.stage_publication_frames(&[proposal_complete_frame_with(
            &start,
            1,
            block_index,
            &call_id,
            tool_id,
            name,
        )]);
        assert!(
            matches!(
                rejected,
                Err(ConversationStoreError::PublicationViolation(_))
            ),
            "mismatched {label} completion was accepted: {rejected:?}"
        );
        store
            .stage_publication_frames(&[proposal_complete_frame_with(
                &start,
                1,
                ContentBlockIndex::new(0),
                &call_id,
                ToolId::new("tool-alpha"),
                "alpha",
            )])
            .expect("exact completion reuses the unchanged sequence");
    }
}

/// A call ID owned by one stream never satisfies a suffix or completion in a
/// different stream. Reusing the provider ID is legal only after the second
/// stream creates its own Started owner.
#[test]
fn proposal_ownership_is_namespaced_by_stream_for_every_staging_transition() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (_first_request, first) = opened_test_stream(&store, "proposal-owner-first");
    let (_second_request, second) = opened_test_stream(&store, "proposal-owner-second");
    let call_id = ToolCallId::new("reused-provider-call");
    store
        .stage_publication_frames(&[proposal_start_frame(&first, 0, &call_id)])
        .expect("first owner");

    for payload in [
        proposal_arguments_frame(&second, 0, ContentBlockIndex::new(0), &call_id, "{}"),
        proposal_complete_frame_with(
            &second,
            0,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        ),
    ] {
        assert!(matches!(
            store.stage_publication_frames(&[payload]),
            Err(ConversationStoreError::PublicationViolation(_))
        ));
    }
    store
        .stage_publication_frames(&[proposal_start_frame(&second, 0, &call_id)])
        .expect("second stream creates its own owner");
    store
        .stage_publication_frames(&[proposal_arguments_frame(
            &second,
            1,
            ContentBlockIndex::new(0),
            &call_id,
            "{}",
        )])
        .expect("second stream arguments");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            &second,
            2,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        )])
        .expect("second stream completion");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            &first,
            1,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        )])
        .expect("first stream still resolves its own owner");
}

/// The same proposal-state validator is used by U. A malformed terminal batch
/// rolls back its frame rows, owner rows, sequence, and terminal marker as one
/// unit; the valid batch can then use the original sequence zero.
#[test]
fn terminal_staging_uses_the_proposal_state_machine_atomically() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (request_id, start) = opened_test_stream(&store, "proposal-terminal");
    commit_provider_outcome(&store, "proposal-terminal", &request_id);
    let before_head = store.load_head().expect("head");
    let before_events = event_count(&store);
    let before_streams = store.load_unsettled_publication_streams().expect("streams");
    let orphan_id = ToolCallId::new("orphan-terminal");
    let rejected = store.commit_publication_terminal(
        &start.stream_id,
        &[
            proposal_start_frame(&start, 0, &ToolCallId::new("valid-terminal")),
            proposal_complete_frame_with(
                &start,
                1,
                ContentBlockIndex::new(0),
                &orphan_id,
                ToolId::new("tool-alpha"),
                "alpha",
            ),
        ],
    );
    assert!(matches!(
        rejected,
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    assert_eq!(store.load_head().expect("head"), before_head);
    assert_eq!(event_count(&store), before_events);
    assert_eq!(
        store.load_unsettled_publication_streams().expect("streams"),
        before_streams
    );
    assert!(
        store
            .load_publication_audit(&start.stream_id)
            .expect("audit")
            .is_none()
    );
    assert!(store.load_canonical().expect("canonical").is_empty());

    let valid_id = ToolCallId::new("valid-terminal");
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[
                proposal_start_frame(&start, 0, &valid_id),
                proposal_complete_frame_with(
                    &start,
                    1,
                    ContentBlockIndex::new(0),
                    &valid_id,
                    ToolId::new("tool-alpha"),
                    "alpha",
                ),
            ],
        )
        .expect("valid terminal batch after rollback");
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("valid audit")
        .0;
    assert_eq!(audit.proposed_call_ids(), vec![valid_id]);
}

/// The former orphan-completion exploit is rejected before it can reach U or
/// an audit. No audited content can therefore be resolved by a later Tool
/// Plane transition under that unowned provider call ID.
#[test]
fn orphan_completion_cannot_become_an_audited_tool_proposal() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (request_id, start) = opened_test_stream(&store, "proposal-orphan-exploit");
    let orphan_id = ToolCallId::new("call-1");
    let rejected = store.stage_publication_frames(&[proposal_complete_frame_with(
        &start,
        0,
        ContentBlockIndex::new(0),
        &orphan_id,
        ToolId::new("tool-alpha"),
        "alpha",
    )]);
    assert!(matches!(
        rejected,
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    commit_provider_outcome(&store, "proposal-orphan-exploit", &request_id);
    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("the empty stream can settle as Incomplete")
        .0;
    assert!(audit.proposed_call_ids().is_empty());
    assert!(
        store
            .load_publication_audit(&start.stream_id)
            .expect("audit")
            .expect("audit exists")
            .proposed_call_ids()
            .is_empty()
    );
    let events_before = event_count(&store);
    let rejected = store.append_event(envelope(
        "orphan-tool-start",
        "proposal-orphan-exploit",
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: orphan_id,
            tool_id: ToolId::new("tool-alpha"),
        },
    ));
    assert!(
        matches!(
            rejected,
            Err(ConversationStoreError::PublicationViolation(ref detail))
                if detail.contains("no durable proposal or canonical Assistant owner")
        ),
        "an unowned orphan call must not be accepted as a no-op: {rejected:?}"
    );
    assert_eq!(event_count(&store), events_before);
}

/// C is bidirectional: a completed durable proposal cannot disappear between
/// publication assembly and the canonical Assistant acceptance. The failed C
/// transaction leaves the stream available for honest audit settlement.
#[test]
fn canonical_acceptance_rejects_an_omitted_completed_proposal_atomically() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (request_id, start) = opened_test_stream(&store, "canonical-omitted");
    let call_id = ToolCallId::new("call-omitted");
    let next = stage_completed_proposal(
        &store,
        &start,
        0,
        ContentBlockIndex::new(0),
        &call_id,
        ToolId::new("tool-alpha"),
        "alpha",
    );
    commit_provider_outcome(&store, "canonical-omitted", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(next, &start, "done")])
        .expect("U");

    let before_head = store.load_head().expect("head");
    let before_events = event_count(&store);
    let before_streams = store.load_unsettled_publication_streams().expect("streams");
    let rejected = store.commit_canonical_publication(
        &start.stream_id,
        &assistant(&start.message_id, "done"),
        committed_event(&start.message_id, "canonical-omitted"),
    );
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("omits Completed proposal")
        ),
        "C accepted an omitted Completed proposal: {rejected:?}"
    );
    assert_eq!(store.load_head().expect("head"), before_head);
    assert_eq!(event_count(&store), before_events);
    assert_eq!(store.load_canonical().expect("canonical").len(), 0);
    assert_eq!(
        store.load_unsettled_publication_streams().expect("streams"),
        before_streams
    );
    assert!(
        store
            .load_publication_audit(&start.stream_id)
            .expect("audit")
            .is_none()
    );

    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("failed C left the proposal settlement untouched")
        .0;
    assert_eq!(audit.proposed_call_ids(), vec![call_id]);
}

/// C must accept the complete proposal set, not merely a valid subset. This
/// also rejects a duplicate/missing ownership projection before any Ledger,
/// Surface, Journal, or settlement mutation can begin.
#[test]
fn canonical_acceptance_rejects_a_strict_proposal_subset_atomically() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (request_id, start) = opened_test_stream(&store, "canonical-subset");
    let first = ToolCallId::new("call-subset-a");
    let second = ToolCallId::new("call-subset-b");
    let next = stage_completed_proposal(
        &store,
        &start,
        0,
        ContentBlockIndex::new(0),
        &first,
        ToolId::new("tool-alpha"),
        "alpha",
    );
    let next = stage_completed_proposal(
        &store,
        &start,
        next,
        ContentBlockIndex::new(1),
        &second,
        ToolId::new("tool-beta"),
        "beta",
    );
    commit_provider_outcome(&store, "canonical-subset", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(next, &start, "done")])
        .expect("U");

    let accepted_subset = assistant_with_tools(
        &start.message_id,
        vec![ToolCall {
            id: first.clone(),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
            arguments: serde_json::json!({}),
        }],
    );
    let before_head = store.load_head().expect("head");
    let before_events = event_count(&store);
    let before_streams = store.load_unsettled_publication_streams().expect("streams");
    let rejected = store.commit_canonical_publication(
        &start.stream_id,
        &accepted_subset,
        committed_event(&start.message_id, "canonical-subset"),
    );
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("omits Completed proposal")
        ),
        "C accepted a strict proposal subset: {rejected:?}"
    );
    assert_eq!(store.load_head().expect("head"), before_head);
    assert_eq!(event_count(&store), before_events);
    assert_eq!(store.load_canonical().expect("canonical").len(), 0);
    assert_eq!(
        store.load_unsettled_publication_streams().expect("streams"),
        before_streams
    );
    assert!(
        store
            .load_publication_audit(&start.stream_id)
            .expect("audit")
            .is_none()
    );

    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("failed C left both proposal owners intact")
        .0;
    assert_eq!(audit.proposed_call_ids(), vec![first, second]);
}

/// A Started-only proposal is not a valid C-side ownership set. It remains an
/// immutable incomplete proposal audit after the rejected canonical attempt.
#[test]
fn canonical_acceptance_rejects_a_started_only_proposal_atomically() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (request_id, start) = opened_test_stream(&store, "canonical-started-only");
    let call_id = ToolCallId::new("call-started-only");
    store
        .stage_publication_frames(&[proposal_start_frame(&start, 0, &call_id)])
        .expect("Started");
    commit_provider_outcome(&store, "canonical-started-only", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(1, &start, "done")])
        .expect("U");

    let before_head = store.load_head().expect("head");
    let before_events = event_count(&store);
    let before_streams = store.load_unsettled_publication_streams().expect("streams");
    let rejected = store.commit_canonical_publication(
        &start.stream_id,
        &assistant(&start.message_id, "done"),
        committed_event(&start.message_id, "canonical-started-only"),
    );
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("Started-only proposal")
        ),
        "C accepted a Started-only proposal: {rejected:?}"
    );
    assert_eq!(store.load_head().expect("head"), before_head);
    assert_eq!(event_count(&store), before_events);
    assert_eq!(store.load_canonical().expect("canonical").len(), 0);
    assert_eq!(
        store.load_unsettled_publication_streams().expect("streams"),
        before_streams
    );
    assert!(
        store
            .load_publication_audit(&start.stream_id)
            .expect("audit")
            .is_none()
    );

    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("failed C left the Started proposal auditable")
        .0;
    assert_eq!(audit.proposed_call_ids(), vec![call_id]);
    assert!(matches!(
        audit.content.as_slice(),
        [
            PublicationAuditBlock::ProposedToolCall {
                complete: false,
                ..
            },
            ..
        ]
    ));
}

/// Tool execution authorization is bound to the frozen tool identity, not
/// merely the provider call ID. The rejected event also leaves `executed=0`:
/// the stream can still be terminalized as an audit in the same transaction
/// boundary that would otherwise detect a leaked execution fact.
#[test]
fn tool_execution_started_rejects_a_foreign_tool_id_atomically() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (request_id, start) = opened_test_stream(&store, "tool-id-mismatch");
    let call_id = ToolCallId::new("call-tool-id-mismatch");
    let next = stage_completed_proposal(
        &store,
        &start,
        0,
        ContentBlockIndex::new(0),
        &call_id,
        ToolId::new("tool-alpha"),
        "alpha",
    );
    commit_provider_outcome(&store, "tool-id-mismatch", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(next, &start, "done")])
        .expect("U");

    let before_head = store.load_head().expect("head");
    let before_events = event_count(&store);
    let before_streams = store.load_unsettled_publication_streams().expect("streams");
    let rejected = store.append_event(envelope(
        "wrong-tool-id-start",
        "tool-id-mismatch",
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: call_id.clone(),
            tool_id: ToolId::new("tool-beta"),
        },
    ));
    assert!(
        matches!(
            &rejected,
            Err(ConversationStoreError::PublicationViolation(detail))
                if detail.contains("tool id")
        ),
        "wrong tool ID was authorized: {rejected:?}"
    );
    assert_eq!(store.load_head().expect("head"), before_head);
    assert_eq!(event_count(&store), before_events);
    assert_eq!(
        store.load_unsettled_publication_streams().expect("streams"),
        before_streams
    );
    assert!(
        store
            .load_publication_audit(&start.stream_id)
            .expect("audit")
            .is_none()
    );

    let audit = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .expect("wrong ToolExecutionStarted left no executed fact")
        .0;
    assert_eq!(audit.proposed_call_ids(), vec![call_id]);
}

/// Two proposals in one canonical Assistant remain independently owned by
/// their `(stream_id, call_id)` rows, including their frozen tool IDs, and can
/// complete the ordinary execution plus atomic `ToolResult` batch path.
#[test]
fn canonical_multiple_proposals_execute_and_commit_results() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let (request_id, start) = opened_test_stream(&store, "canonical-multiple");
    let first = ToolCallId::new("call-multiple-a");
    let second = ToolCallId::new("call-multiple-b");
    let next = stage_completed_proposal(
        &store,
        &start,
        0,
        ContentBlockIndex::new(0),
        &first,
        ToolId::new("tool-alpha"),
        "alpha",
    );
    let next = stage_completed_proposal(
        &store,
        &start,
        next,
        ContentBlockIndex::new(1),
        &second,
        ToolId::new("tool-beta"),
        "beta",
    );
    commit_provider_outcome(&store, "canonical-multiple", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(next, &start, "done")])
        .expect("U");
    let assistant = assistant_with_tools(
        &start.message_id,
        vec![
            ToolCall {
                id: first.clone(),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
                arguments: serde_json::json!({}),
            },
            ToolCall {
                id: second.clone(),
                tool_id: ToolId::new("tool-beta"),
                name: "beta".to_owned(),
                arguments: serde_json::json!({}),
            },
        ],
    );
    store
        .commit_canonical_publication(
            &start.stream_id,
            &assistant,
            committed_event(&start.message_id, "canonical-multiple"),
        )
        .expect("C");

    let before_wrong_tool_event = event_count(&store);
    let rejected = store.append_event(envelope(
        "multiple-wrong-tool-id",
        "canonical-multiple",
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: first.clone(),
            tool_id: ToolId::new("tool-beta"),
        },
    ));
    assert!(
        matches!(
            rejected,
            Err(ConversationStoreError::PublicationViolation(_))
        ),
        "canonical owner accepted a foreign tool ID"
    );
    assert_eq!(event_count(&store), before_wrong_tool_event);

    for (event_id, call_id, tool_id) in [
        ("multiple-start-a", first.clone(), ToolId::new("tool-alpha")),
        ("multiple-start-b", second.clone(), ToolId::new("tool-beta")),
    ] {
        store
            .append_event(envelope(
                event_id,
                "canonical-multiple",
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call_id,
                    tool_id,
                },
            ))
            .expect("ToolExecutionStarted");
    }
    let first_result =
        tool_message_with_tool("multiple-result-a", &first, ToolId::new("tool-alpha"));
    let second_result =
        tool_message_with_tool("multiple-result-b", &second, ToolId::new("tool-beta"));
    store
        .append_canonical_batch_with_events(
            &[first_result, second_result],
            &[
                envelope(
                    "multiple-result-event-a",
                    "canonical-multiple",
                    RuntimeEvent::ToolMessageCommitted {
                        message_id: MessageId::new("multiple-result-a"),
                        tool_call_id: first,
                    },
                ),
                envelope(
                    "multiple-result-event-b",
                    "canonical-multiple",
                    RuntimeEvent::ToolMessageCommitted {
                        message_id: MessageId::new("multiple-result-b"),
                        tool_call_id: second,
                    },
                ),
            ],
        )
        .expect("atomic multi ToolResult batch");
    assert_eq!(store.load_canonical().expect("canonical").len(), 3);
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
        .expect("audit")
        .0;
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
        .expect("audit")
        .0;
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
            tool_call_id: call_id.clone(),
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
    assert_eq!(audit.0.kind, PublicationAuditKind::Unaccepted);
    assert!(matches!(
        &audit.0.content[0],
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

/// Every dependent Tool Plane transition is owned by the same durable
/// proposal guard. An audit may not be bypassed by writing an outcome,
/// canonical `ToolResult`, batched `ToolResult`, or detached authorization fact
/// through a different store API.
#[test]
fn audited_proposals_reject_all_dependent_tool_transitions_atomically() {
    for reaches_u in [false, true] {
        let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
        store.initialize(&[]).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "msg-1");
        let call_id = ToolCallId::new("call-audited");
        store.open_publication_stream(&start).expect("open");
        store
            .stage_publication_frames(&[proposal_start_frame(&start, 0, &call_id)])
            .expect("proposal registration");
        if reaches_u {
            commit_provider_outcome(&store, "1", &request_id);
            store
                .commit_publication_terminal(&start.stream_id, &[text(1, &start, "done")])
                .expect("U");
        }
        let expected_kind = if reaches_u {
            PublicationAuditKind::Unaccepted
        } else {
            PublicationAuditKind::Incomplete
        };
        let audit = store
            .terminalize_publication_audit(&start.stream_id, fixed_time())
            .expect("audit")
            .0;
        assert_eq!(audit.kind, expected_kind);

        let assert_unchanged = |before_head: &rustx::durable::DurableConversationHead,
                                before_events: usize,
                                label: &str| {
            assert_eq!(
                store.load_head().expect("head"),
                *before_head,
                "{label}: head changed"
            );
            assert_eq!(
                store.load_canonical().expect("canonical").len(),
                0,
                "{label}: Ledger changed"
            );
            assert_eq!(
                store.read_events(None, 256).expect("events").events.len(),
                before_events,
                "{label}: Journal changed"
            );
            assert_eq!(
                store
                    .load_unsettled_publication_streams()
                    .expect("streams")
                    .len(),
                0,
                "{label}: stream changed"
            );
            assert_eq!(
                store
                    .load_publication_audit(&start.stream_id)
                    .expect("audit"),
                Some(audit.clone()),
                "{label}: audit changed"
            );
        };

        let before_head = store.load_head().expect("head");
        let before_events = store.read_events(None, 256).expect("events").events.len();
        let dependent_events = [
            envelope(
                "audited-start",
                "1",
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call_id.clone(),
                    tool_id: ToolId::new("tool-alpha"),
                },
            ),
            envelope(
                "audited-progress",
                "1",
                RuntimeEvent::ToolExecutionProgress {
                    tool_call_id: call_id.clone(),
                    tool_id: ToolId::new("tool-alpha"),
                    execution_id: None,
                    progress: rustx::tools::types::ToolProgress::default(),
                },
            ),
            envelope(
                "audited-completed",
                "1",
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: call_id.clone(),
                    tool_id: ToolId::new("tool-alpha"),
                    result: ToolExecutionResult {
                        status: ToolExecutionStatus::Success,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: Some(0),
                        artifacts: Vec::new(),
                        truncation: None,
                        managed_output: None,
                    },
                },
            ),
            envelope(
                "audited-failed",
                "1",
                RuntimeEvent::ToolExecutionFailed {
                    tool_call_id: call_id.clone(),
                    tool_id: ToolId::new("tool-alpha"),
                    error: "failed".to_owned(),
                },
            ),
        ];
        for event in dependent_events {
            let rejected = store.append_event(event);
            assert!(
                matches!(
                    rejected,
                    Err(ConversationStoreError::PublicationViolation(_))
                ),
                "audited proposal accepted a dependent event: {rejected:?}"
            );
            assert_unchanged(&before_head, before_events, "dependent event");
        }

        let single = tool_message("tool-result-single", &call_id);
        let single_event = envelope(
            "audited-tool-message",
            "1",
            RuntimeEvent::ToolMessageCommitted {
                message_id: MessageId::new("tool-result-single"),
                tool_call_id: call_id.clone(),
            },
        );
        let rejected = store.append_canonical_with_event(&single, single_event);
        assert!(
            matches!(
                rejected,
                Err(ConversationStoreError::PublicationViolation(_))
            ),
            "audited proposal accepted a single ToolResult: {rejected:?}"
        );
        assert_unchanged(&before_head, before_events, "single ToolResult");

        let batch = [
            tool_message("tool-result-batch-a", &call_id),
            tool_message("tool-result-batch-b", &call_id),
        ];
        let batch_events = [
            envelope(
                "audited-tool-batch-a",
                "1",
                RuntimeEvent::ToolMessageCommitted {
                    message_id: MessageId::new("tool-result-batch-a"),
                    tool_call_id: call_id.clone(),
                },
            ),
            envelope(
                "audited-tool-batch-b",
                "1",
                RuntimeEvent::ToolMessageCommitted {
                    message_id: MessageId::new("tool-result-batch-b"),
                    tool_call_id: call_id.clone(),
                },
            ),
        ];
        let rejected = store.append_canonical_batch_with_events(&batch, &batch_events);
        assert!(
            matches!(
                rejected,
                Err(ConversationStoreError::PublicationViolation(_))
            ),
            "audited proposal accepted a ToolResult batch: {rejected:?}"
        );
        assert_unchanged(&before_head, before_events, "ToolResult batch");

        let mut background = envelope(
            "audited-background",
            "1",
            RuntimeEvent::BackgroundExecutionCommitted {
                execution_id: rustx::runtime::identity::ToolExecutionId::new("exec-audited"),
                tool_call_id: call_id.clone(),
                tool_id: ToolId::new("tool-alpha"),
                tool_name: "alpha".to_owned(),
            },
        );
        background.attempt_id = None;
        background.turn_id = None;
        let rejected = store.append_event(background);
        assert!(
            matches!(
                rejected,
                Err(ConversationStoreError::PublicationViolation(_))
            ),
            "audited proposal accepted background authorization: {rejected:?}"
        );
        assert_unchanged(&before_head, before_events, "background authorization");

        let subagent_id = SubagentId::new("subagent-audited");
        let mut subagent = envelope(
            "audited-subagent",
            "1",
            RuntimeEvent::SubagentOwnershipCommitted {
                subagent_id: subagent_id.clone(),
                child_agent_id: AgentId::new("child-audited"),
                child_conversation_id: ConversationId::new("child-conversation"),
                tool_call_id: call_id,
                agent: "profile".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                workspace: rustx::runtime::subagent::WorkspaceSnapshot::shared(
                    std::path::PathBuf::from("<shared-workspace>"),
                ),
            },
        );
        subagent.event_id = EventId::new(format!("subagent-committed-event:{subagent_id}"));
        subagent.attempt_id = None;
        subagent.turn_id = None;
        let rejected = store.append_event(subagent);
        assert!(
            matches!(
                rejected,
                Err(ConversationStoreError::PublicationViolation(_))
            ),
            "audited proposal accepted subagent authorization: {rejected:?}"
        );
        assert_unchanged(&before_head, before_events, "subagent authorization");
    }
}

/// A proposal owned by a canonical Assistant may take the normal execution
/// and `ToolResult` path, including the same policy-result representation used
/// for denied/cancelled slots.
#[test]
fn canonical_proposal_can_execute_and_commit_its_tool_result() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    let call_id = ToolCallId::new("call-canonical");
    store.open_publication_stream(&start).expect("open");
    store
        .stage_publication_frames(&[proposal_start_frame(&start, 0, &call_id)])
        .expect("proposal");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            &start,
            1,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        )])
        .expect("complete proposal");
    commit_provider_outcome(&store, "1", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(2, &start, "done")])
        .expect("U");
    let assistant = assistant_with_tool(&start.message_id, &call_id);
    store
        .commit_canonical_publication(
            &start.stream_id,
            &assistant,
            committed_event(&start.message_id, "1"),
        )
        .expect("C");

    store
        .append_event(envelope(
            "canonical-tool-start",
            "1",
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_id.clone(),
                tool_id: ToolId::new("tool-alpha"),
            },
        ))
        .expect("tool start");
    store
        .append_event(envelope(
            "canonical-tool-completed",
            "1",
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call_id.clone(),
                tool_id: ToolId::new("tool-alpha"),
                result: ToolExecutionResult {
                    status: ToolExecutionStatus::Denied {
                        reason: "policy".to_owned(),
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    managed_output: None,
                },
            },
        ))
        .expect("tool outcome");
    let result = tool_message("canonical-tool-result", &call_id);
    store
        .append_canonical_with_event(
            &result,
            envelope(
                "canonical-tool-message",
                "1",
                RuntimeEvent::ToolMessageCommitted {
                    message_id: MessageId::new("canonical-tool-result"),
                    tool_call_id: call_id,
                },
            ),
        )
        .expect("canonical ToolResult");
    assert_eq!(store.load_canonical().expect("canonical").len(), 2);
}

/// Startup repair of a genuinely canonical Assistant turn is still legal: the
/// retained composite proposal row resolves the recovery-generated `ToolResult`
/// to the canonical stream, while an audited proposal would be rejected.
#[test]
fn recovery_repairs_a_canonical_proposal_without_audit_aliasing() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("attempt-start-repair"),
            sequence: 0,
            conversation_id: conversation_id(),
            attempt_id: Some(attempt()),
            turn_id: None,
            timestamp: fixed_time(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        })
        .expect("attempt start");
    let request_id = start_request(&store, "1");
    let start = stream_start(&request_id, "1", "msg-1");
    let call_id = ToolCallId::new("call-repair");
    store.open_publication_stream(&start).expect("open");
    store
        .stage_publication_frames(&[proposal_start_frame(&start, 0, &call_id)])
        .expect("proposal");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            &start,
            1,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        )])
        .expect("complete proposal");
    commit_provider_outcome(&store, "1", &request_id);
    store
        .commit_publication_terminal(&start.stream_id, &[text(2, &start, "done")])
        .expect("U");
    store
        .commit_canonical_publication(
            &start.stream_id,
            &assistant_with_tool(&start.message_id, &call_id),
            committed_event(&start.message_id, "1"),
        )
        .expect("C");
    store
        .append_event(envelope(
            "repair-tool-start",
            "1",
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_id.clone(),
                tool_id: ToolId::new("tool-alpha"),
            },
        ))
        .expect("tool start");

    let report = recover(&store, &FixedClock).expect("recovery");
    assert_eq!(report.reconciliation().repaired_tool_results, vec![call_id]);
    assert!(store.load_canonical().expect("canonical").iter().any(|message| {
        matches!(message, MessageBlock::Tool(tool) if tool.tool_call_id == ToolCallId::new("call-repair"))
    }));
}

/// Provider reuse of a `ToolCallId` is legal across publication generations,
/// but the durable owner is never selected by bare call id. The exact turn
/// resolves the accepted proposal; an event for the audited generation is
/// rejected.
#[test]
fn reused_tool_call_id_keeps_publication_ownership_exact() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let call_id = ToolCallId::new("provider-reused-call");

    let first_request = start_request(&store, "1");
    let first = stream_start(&first_request, "1", "msg-1");
    store.open_publication_stream(&first).expect("first open");
    store
        .stage_publication_frames(&[proposal_start_frame(&first, 0, &call_id)])
        .expect("first proposal");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            &first,
            1,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        )])
        .expect("first complete proposal");
    commit_provider_outcome(&store, "1", &first_request);
    store
        .commit_publication_terminal(&first.stream_id, &[text(2, &first, "first")])
        .expect("first U");
    store
        .terminalize_publication_audit(&first.stream_id, fixed_time())
        .expect("first audit");

    let second_request = start_request(&store, "2");
    let second = stream_start(&second_request, "2", "msg-2");
    store.open_publication_stream(&second).expect("second open");
    store
        .stage_publication_frames(&[proposal_start_frame(&second, 0, &call_id)])
        .expect("same provider call id has a distinct owner");
    store
        .stage_publication_frames(&[proposal_complete_frame_with(
            &second,
            1,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            "alpha",
        )])
        .expect("second complete proposal");
    commit_provider_outcome(&store, "2", &second_request);
    store
        .commit_publication_terminal(&second.stream_id, &[text(2, &second, "second")])
        .expect("second U");
    store
        .commit_canonical_publication(
            &second.stream_id,
            &assistant_with_tool(&second.message_id, &call_id),
            committed_event(&second.message_id, "2"),
        )
        .expect("second C");

    store
        .append_event(envelope(
            "reused-call-second-start",
            "2",
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call_id.clone(),
                tool_id: ToolId::new("tool-alpha"),
            },
        ))
        .expect("the accepted second proposal resolves exactly");
    let rejected = store.append_event(envelope(
        "reused-call-first-start",
        "1",
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: call_id,
            tool_id: ToolId::new("tool-alpha"),
        },
    ));
    assert!(
        matches!(rejected, Err(ConversationStoreError::PublicationViolation(ref detail)) if detail.contains("may never execute")),
        "the audited first owner must remain forbidden: {rejected:?}"
    );
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
                if detail.contains("exact Request Snapshot generation")
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

/// Opening, provider P, and canonical C each re-prove the exact durable
/// generation. Foreign request/attempt/turn/message tuples fail before their
/// transition can change Ledger, Surface, Journal, lifecycle, or publication
/// state.
#[test]
fn publication_generation_rejections_are_side_effect_free() {
    let store = SqliteConversationStore::in_memory(conversation_id()).expect("store");
    store.initialize(&[]).expect("initialize");
    let request_id = start_request(&store, "1");
    let valid = stream_start(&request_id, "1", "msg-1");
    let foreign_request_id = start_request(&store, "2");
    let before_head = store.load_head().expect("head");
    let before_events = store.read_events(None, 256).expect("events").events.len();

    let missing = stream_start(&RequestId::new("request-missing"), "1", "msg-1");
    assert!(matches!(
        store.open_publication_stream(&missing),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    let mut foreign_request = valid.clone();
    foreign_request.request_id = foreign_request_id;
    assert!(matches!(
        store.open_publication_stream(&foreign_request),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    let mut foreign_attempt = valid.clone();
    foreign_attempt.attempt_id = AttemptId::new("foreign-attempt");
    assert!(matches!(
        store.open_publication_stream(&foreign_attempt),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    let mut foreign_turn = valid.clone();
    foreign_turn.turn_id = TurnId::new("foreign-turn");
    assert!(matches!(
        store.open_publication_stream(&foreign_turn),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    assert_eq!(store.load_head().expect("head"), before_head);
    assert_eq!(
        store.read_events(None, 256).expect("events").events.len(),
        before_events
    );
    assert!(
        store
            .load_unsettled_publication_streams()
            .expect("streams")
            .is_empty()
    );

    store.open_publication_stream(&valid).expect("valid open");
    let provider_before = store.read_events(None, 256).expect("events").events.len();
    let mut foreign_provider_attempt = envelope(
        "foreign-provider-attempt",
        "1",
        RuntimeEvent::ModelRequestCompleted {
            request_id: request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    );
    foreign_provider_attempt.attempt_id = Some(AttemptId::new("foreign-attempt"));
    assert!(matches!(
        store.append_event(foreign_provider_attempt),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    let mut foreign_provider_turn = envelope(
        "foreign-provider-turn",
        "1",
        RuntimeEvent::ModelRequestCompleted {
            request_id: request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    );
    foreign_provider_turn.turn_id = Some(TurnId::new("foreign-turn"));
    assert!(matches!(
        store.append_event(foreign_provider_turn),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(
        store.read_events(None, 256).expect("events").events.len(),
        provider_before
    );
    commit_provider_outcome(&store, "1", &request_id);
    let after_provider = store.read_events(None, 256).expect("events").events.len();
    let duplicate = store.append_event(envelope(
        "duplicate-provider-outcome",
        "1",
        RuntimeEvent::ModelRequestCompleted {
            request_id: request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        },
    ));
    assert!(matches!(
        duplicate,
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    let contradictory = store.append_event(envelope(
        "contradictory-provider-outcome",
        "1",
        RuntimeEvent::ModelRequestFailed {
            request_id: request_id.clone(),
            error: rustx::model::ModelError {
                kind: rustx::model::ModelErrorKind::ProviderError,
                message: "contradictory".to_owned(),
                retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
                retry_after_ms: None,
                provider_code: None,
                context_overflow: None,
            },
            usage: None,
        },
    ));
    assert!(matches!(
        contradictory,
        Err(ConversationStoreError::TerminalViolation(_))
    ));
    assert_eq!(
        store.read_events(None, 256).expect("events").events.len(),
        after_provider
    );
    store
        .commit_publication_terminal(&valid.stream_id, &[text(0, &valid, "answer")])
        .expect("U");

    let exact_assistant = assistant(&valid.message_id, "answer");
    let c_head = store.load_head().expect("head");
    let c_events = store.read_events(None, 256).expect("events").events.len();
    let mut foreign_c_attempt = committed_event(&valid.message_id, "1");
    foreign_c_attempt.attempt_id = Some(AttemptId::new("foreign-attempt"));
    assert!(matches!(
        store.commit_canonical_publication(&valid.stream_id, &exact_assistant, foreign_c_attempt),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    let mut foreign_c_turn = committed_event(&valid.message_id, "1");
    foreign_c_turn.turn_id = Some(TurnId::new("foreign-turn"));
    assert!(matches!(
        store.commit_canonical_publication(&valid.stream_id, &exact_assistant, foreign_c_turn),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    let foreign_message_id = MessageId::new("foreign-message");
    assert!(matches!(
        store.commit_canonical_publication(
            &valid.stream_id,
            &assistant(&foreign_message_id, "foreign"),
            committed_event(&foreign_message_id, "1"),
        ),
        Err(ConversationStoreError::PublicationViolation(_))
    ));
    assert_eq!(store.load_head().expect("head"), c_head);
    assert_eq!(
        store.read_events(None, 256).expect("events").events.len(),
        c_events
    );
    assert!(store.load_canonical().expect("canonical").is_empty());
    store
        .commit_canonical_publication(
            &valid.stream_id,
            &exact_assistant,
            committed_event(&valid.message_id, "1"),
        )
        .expect("exact P -> U -> C generation");
    assert_eq!(store.load_canonical().expect("canonical").len(), 1);
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
