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
//! `tests/scripted/agent/publication.rs`, because it needs the scripted
//! model adapter.

#![allow(clippy::too_many_lines)] // deterministic store scenarios stay linear

use chrono::{DateTime, TimeZone, Utc};
use rustx::context::ContextGeneration;
use rustx::durable::{
    CanonicalStore, JournalStore, ObservationStore, ProjectionStore, PublicationStore, RequestStore,
};
use rustx::durable::{ConversationStoreError, SqliteConversationStore};
use rustx::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
    ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
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

const CONVERSATION: &str = "conv_601cccec-75c7-703d-af18-cdf2cb42a8a1";

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
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new(turn),
            retry_number: 0,
        },
        rustx::context::resources::LedgerFrontier((head.revision).get()),
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
    crate::freeze_request(store, &mut snapshot);
    store
        .commit_model_turn_start(&[], &snapshot, fixed_time())
        .expect("request start");
    request_id
}

fn envelope(event_id: &str, turn: &str, event: RuntimeEvent) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
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
                generation: None,
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

fn tool_message(message_id: &str, owner: &MessageId, call_id: &ToolCallId) -> MessageBlock {
    tool_message_with_tool(message_id, owner, 0, call_id, ToolId::new("tool-alpha"))
}

fn tool_message_with_tool(
    message_id: &str,
    owner: &MessageId,
    index: u32,
    call_id: &ToolCallId,
    tool_id: ToolId,
) -> MessageBlock {
    MessageBlock::Tool(ToolMessageBlock {
        occurrence: rustx::message::types::ToolCallOccurrenceRef::new(
            owner.clone(),
            rustx::message::types::ContentBlockIndex::new(index),
        ),
        id: MessageId::new(message_id),
        tool_call_id: call_id.clone(),
        tool_id,
        result: successful_tool_result(),
    })
}

fn successful_tool_result() -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Success,
        content: Vec::new(),
        duration_ms: 1,
        exit_code: Some(0),
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
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
    let first_result = tool_message_with_tool(
        "multiple-result-a",
        &start.message_id,
        0,
        &first,
        ToolId::new("tool-alpha"),
    );
    let second_result = tool_message_with_tool(
        "multiple-result-b",
        &start.message_id,
        1,
        &second,
        ToolId::new("tool-beta"),
    );
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

/// A restart can extend the transcript with a noncanonical audit while leaving
/// canonical history byte-identical. A configuration/restart preservation test
/// must compare the Ledger, not equate this broader read model with history.
#[test]
fn recovery_audit_extends_transcript_without_rewriting_canonical_history() {
    let durable = Durable::new();
    let canonical = vec![assistant(
        &MessageId::new("earlier-answer"),
        "canonical work",
    )];
    let (before, stream_id) = {
        let store = durable.open();
        store.initialize(&canonical).expect("initialize");
        let request_id = start_request(&store, "1");
        let start = stream_start(&request_id, "1", "unfinished-answer");
        store.open_publication_stream(&start).expect("open");
        // Exact durable prefix: a started publication without frames or U/C,
        // matching the empty incomplete audit observed in the macOS failure.
        (
            store.load_transcript_page(None, 64).expect("before"),
            start.stream_id,
        )
    };
    assert_eq!(before.entries.len(), 1);

    let report = recover_reopened(&durable);
    assert_eq!(report.publication_classes().len(), 1);
    let store = durable.open();
    let after = store.load_transcript_page(None, 64).expect("after");
    assert_eq!(after.entries.len(), 2);
    assert_eq!(after.entries[0], before.entries[0]);
    assert!(matches!(
        &after.entries[1].item,
        rustx::durable::TranscriptItem::PublicationAudit { audit }
            if audit.stream_id == stream_id
                && audit.kind == PublicationAuditKind::Incomplete
                && audit.content.is_empty()
    ));
    assert_eq!(
        serde_json::to_vec(&store.load_canonical().expect("canonical history")).unwrap(),
        serde_json::to_vec(&canonical).unwrap(),
    );
    drop(store);
    assert!(recover_reopened(&durable).publication_classes().is_empty());
    assert_eq!(
        durable
            .open()
            .load_transcript_page(None, 64)
            .expect("repeated restart")
            .entries,
        after.entries,
    );
}

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
                        deliveries: Vec::new(),
                        status: ToolExecutionStatus::Success,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: Some(0),
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
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

        let single = tool_message("tool-result-single", &start.message_id, &call_id);
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
            matches!(rejected, Err(ConversationStoreError::InvalidReference(_))),
            "audited proposal accepted a single ToolResult: {rejected:?}"
        );
        assert_unchanged(&before_head, before_events, "single ToolResult");

        let batch = [
            tool_message("tool-result-batch-a", &start.message_id, &call_id),
            tool_message("tool-result-batch-b", &start.message_id, &call_id),
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
            matches!(rejected, Err(ConversationStoreError::InvalidReference(_))),
            "audited proposal accepted a ToolResult batch: {rejected:?}"
        );
        assert_unchanged(&before_head, before_events, "ToolResult batch");

        let mut background = envelope(
            "audited-background",
            "1",
            RuntimeEvent::BackgroundExecutionCommitted {
                execution_id: rustx::runtime::identity::ToolExecutionId::new(
                    "exec_deb98904-49c7-7914-8acf-a96b97f78e4b",
                ),
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
                parent_agent_id: rustx::runtime::identity::AgentId::new("agent-parent"),
                admitted_authority: None,
                subagent_id: subagent_id.clone(),
                child_agent_id: AgentId::new("child-audited"),
                child_conversation_id: ConversationId::new(
                    "conv_b520d0ec-730a-7133-8d51-7de417e31c5a",
                ),
                origin: rustx::runtime::subagent::AgentActivationOrigin::CreationTool {
                    tool_call_id: call_id,
                },
                agent: "profile".to_owned(),
                definition_digest: "sha256:definition".to_owned(),
                profile_digest: "sha256:profile".to_owned(),
                ownership: rustx::events::types::SubagentOwnershipKind::Normal,
                workspace: std::path::PathBuf::from("/shared-workspace"),
            },
        );
        subagent.event_id = EventId::new(format!("subagent-committed-event:{subagent_id}"));
        subagent.attempt_id = None;
        subagent.turn_id = None;
        let (subagent, authority) = crate::agent_authority::admit_agent(subagent);
        let rejected = store.append_agent_admission(subagent, &authority);
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
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Denied {
                        reason: "policy".to_owned(),
                    },
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            },
        ))
        .expect("tool outcome");
    let result = tool_message("canonical-tool-result", &start.message_id, &call_id);
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
            tool_occurrence: None,
            observation: None,
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
            generation: None,
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
            generation: None,
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
            generation: None,
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
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
            usage: None,
            generation: None,
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
            generation: None,
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
                generation: None,
            },
        )),
        Err(ConversationStoreError::TerminalViolation(_))
    ));
}

// Issue #467 byte-observation contracts. Real file-backed SQLite and reopen
// are the boundary; staging/C/audit transactions are the linearization points.
fn live_content_source(
    start: &PublicationStreamStart,
    block: u32,
) -> rustx::observation::content::ContentSource {
    rustx::observation::content::ContentSource::LivePublicationPrefix {
        component: rustx::observation::content::PublicationContentComponent::Payload,
        stream_id: start.stream_id.clone(),
        message_id: start.message_id.clone(),
        attempt_id: start.attempt_id.clone(),
        turn_id: start.turn_id.clone(),
        publication_request_id: start.request_id.clone(),
        block_index: ContentBlockIndex::new(block),
    }
}
fn read_content_bytes(
    store: &SqliteConversationStore,
    reference: &rustx::observation::content::ContentRef,
) -> Vec<u8> {
    use base64::Engine;
    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        let chunk = store
            .content_read(reference, offset, 65_536)
            .expect("bounded byte read");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_base64)
            .expect("base64");
        assert!(decoded.len() <= 65_536);
        assert_eq!(chunk.end_offset, offset + decoded.len() as u64);
        bytes.extend(decoded);
        if chunk.complete {
            assert_eq!(chunk.next_offset, None);
            break;
        }
        assert!(chunk.end_offset > offset);
        offset = chunk.next_offset.expect("advancing continuation");
    }
    bytes
}
#[test]
fn t08_large_canonical_content_exact_byte_chunks_survive_restart() {
    use rustx::observation::content::{ContentRepresentation as Rep, ContentSource};
    let durable = Durable::new();
    // Every chunk boundary crosses an actual multibyte character.
    let original = format!("a{}", "水🙂".repeat(600_000));
    let message_id = MessageId::new("large-canonical");
    {
        let store = durable.open();
        store
            .initialize(&[assistant(&message_id, &original)])
            .expect("canonical commit");
    }
    let store = durable.open();
    let reference = store
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                component: rustx::observation::content::CanonicalContentComponent::Payload,
                locator: rustx::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id,
                },
                block_index: ContentBlockIndex::new(0),
            },
            Rep::Utf8Text,
        )
        .unwrap()
        .unwrap();
    assert_eq!(reference.readable_end, original.len() as u64);
    assert_eq!(read_content_bytes(&store, &reference), original.as_bytes());
    assert!(store.content_read(&reference, 0, 65_537).is_err());
    assert!(
        store
            .content_read(&reference, 9_007_199_254_740_992, 1)
            .is_err()
    );
    assert!(
        store
            .content_read(&reference, reference.readable_end + 1, 1)
            .is_err()
    );
    assert!(store.content_read(&reference, 0, 0).is_err());
}
#[test]
fn t09_t12_t19_committed_live_prefix_fixed_identity_and_frontier() {
    use rustx::observation::content::ContentRepresentation as Rep;
    let durable = Durable::new();
    let (start, reference) = {
        let store = durable.open();
        store.initialize(&[]).unwrap();
        let (_, start) = opened_test_stream(&store, "1");
        store
            .stage_publication_frames(&[text(0, &start, "α")])
            .unwrap();
        let reference = store
            .content_descriptor(&live_content_source(&start, 0), Rep::Utf8Text)
            .unwrap()
            .unwrap();
        // A refused transaction cannot create readable candidate bytes.
        assert!(
            store
                .stage_publication_frames(&[text(2, &start, "secret")])
                .is_err()
        );
        assert_eq!(
            store
                .content_descriptor(&live_content_source(&start, 0), Rep::Utf8Text)
                .unwrap()
                .unwrap(),
            reference
        );
        // Fail after the transaction has physically indexed the new bytes.
        // The publication-frame insert is the deterministic failure boundary;
        // rollback must remove both the byte tail and its frontier evidence.
        let raw_db = rusqlite::Connection::open(&durable.path).unwrap();
        raw_db.execute_batch("CREATE TRIGGER fail_frame BEFORE INSERT ON publication_frames BEGIN SELECT RAISE(ABORT,'staging failure'); END;").unwrap();
        assert!(
            store
                .stage_publication_frames(&[text(1, &start, "hidden")])
                .is_err()
        );
        assert_eq!(
            store
                .content_descriptor(&live_content_source(&start, 0), Rep::Utf8Text)
                .unwrap()
                .unwrap(),
            reference
        );
        assert_eq!(read_content_bytes(&store, &reference), "α".as_bytes());
        raw_db.execute_batch("DROP TRIGGER fail_frame;").unwrap();
        store
            .stage_publication_frames(&[text(1, &start, "🙂")])
            .unwrap();
        assert_eq!(read_content_bytes(&store, &reference), "α".as_bytes());
        (start, reference)
    };
    let store = durable.open();
    assert_eq!(read_content_bytes(&store, &reference), "α".as_bytes());
    let latest = store
        .content_descriptor(&live_content_source(&start, 0), Rep::Utf8Text)
        .unwrap()
        .unwrap();
    assert_eq!(read_content_bytes(&store, &latest), "α🙂".as_bytes());
    for mutate in 0..5 {
        let mut forged = reference.clone();
        match mutate {
            0 => forged.readable_end = latest.readable_end,
            1 => forged.content_revision = latest.content_revision,
            2 => forged.representation = Rep::RawJsonUtf8,
            3 => {
                forged.conversation_id =
                    ConversationId::new("conv_601cccec-75c7-703d-af18-cdf2cb42a8a3");
            }
            _ => {
                if let rustx::observation::content::ContentSource::LivePublicationPrefix {
                    attempt_id,
                    ..
                } = &mut forged.source
                {
                    *attempt_id = AttemptId::new("foreign-attempt");
                }
            }
        }
        assert!(
            store.content_read(&forged, 0, 1).is_err(),
            "forged evidence {mutate}"
        );
    }
    // Pure reads preserve all execution/publication facts.
    let facts = event_count(&store);
    let streams = store.load_unsettled_publication_streams().unwrap();
    store.content_read(&latest, 1, 1).unwrap();
    assert_eq!(event_count(&store), facts);
    assert_eq!(store.load_unsettled_publication_streams().unwrap(), streams);
    let (audit, _) = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .unwrap();
    assert!(
        store.content_read(&reference, 0, 1).is_err(),
        "expired live ref must not redirect"
    );
    let source = rustx::observation::content::ContentSource::DurableTranscriptAudit {
        component: rustx::observation::content::PublicationContentComponent::Payload,
        stream_id: audit.stream_id,
        message_id: audit.message_id,
        attempt_id: audit.attempt_id,
        turn_id: audit.turn_id,
        publication_request_id: audit.request_id,
        block_index: ContentBlockIndex::new(0),
    };
    let retained = store
        .content_descriptor(&source, Rep::Utf8Text)
        .unwrap()
        .unwrap();
    drop(store);
    assert_eq!(
        read_content_bytes(&durable.open(), &retained),
        "α🙂".as_bytes()
    );
}
#[test]
fn t19_raw_tool_json_retains_original_bytes_after_canonical_commit() {
    use rustx::observation::content::{ContentRepresentation as Rep, ContentSource};
    let durable = Durable::new();
    let raw = "{ \n}\t";
    let (message_id, live) = {
        let store = durable.open();
        store.initialize(&[]).unwrap();
        let (request, start) = opened_test_stream(&store, "1");
        let call = ToolCallId::new("raw-call");
        store
            .stage_publication_frames(&[proposal_start_frame(&start, 0, &call)])
            .unwrap();
        store
            .stage_publication_frames(&[proposal_arguments_frame(
                &start,
                1,
                ContentBlockIndex::new(0),
                &call,
                raw,
            )])
            .unwrap();
        let live = store
            .content_descriptor(&live_content_source(&start, 0), Rep::RawJsonUtf8)
            .unwrap()
            .unwrap();
        assert_eq!(read_content_bytes(&store, &live), raw.as_bytes());
        store
            .stage_publication_frames(&[proposal_complete_frame_with(
                &start,
                2,
                ContentBlockIndex::new(0),
                &call,
                ToolId::new("tool-alpha"),
                "alpha",
            )])
            .unwrap();
        commit_provider_outcome(&store, "1", &request);
        store
            .commit_publication_terminal(
                &start.stream_id,
                &[frame(&start, 3, PublicationPayload::TerminalOnly)],
            )
            .unwrap();
        let message = assistant_with_tool(&start.message_id, &call);
        store
            .commit_canonical_publication(
                &start.stream_id,
                &message,
                committed_event(&start.message_id, "1"),
            )
            .unwrap();
        (start.message_id, live)
    };
    let store = durable.open();
    assert!(store.content_read(&live, 0, 1).is_err());
    let source = ContentSource::CanonicalMessageBlock {
        component: rustx::observation::content::CanonicalContentComponent::Payload,
        locator: rustx::observation::content::CanonicalContentLocator::ByMessageId { message_id },
        block_index: ContentBlockIndex::new(0),
    };
    let raw_ref = store
        .content_descriptor(&source, Rep::RawJsonUtf8)
        .unwrap()
        .unwrap();
    assert_eq!(read_content_bytes(&store, &raw_ref), raw.as_bytes());
    let structural = store
        .content_descriptor(&source, Rep::CanonicalJsonV1)
        .unwrap()
        .unwrap();
    assert_eq!(read_content_bytes(&store, &structural), b"{}");
    assert!(
        store
            .content_descriptor(&source, Rep::BinaryBytes)
            .unwrap()
            .is_none(),
        "artifact references do not grant transcript binary bytes"
    );
}

#[test]
fn t19_parsed_only_tool_value_never_invents_canonical_raw_json() {
    use rustx::observation::content::{ContentRepresentation as Rep, ContentSource};
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (request, start) = opened_test_stream(&store, "1");
    let call = ToolCallId::new("parsed-only-call");
    store
        .stage_publication_frames(&[
            proposal_start_frame(&start, 0, &call),
            proposal_complete_frame_with(
                &start,
                1,
                ContentBlockIndex::new(0),
                &call,
                ToolId::new("tool-alpha"),
                "alpha",
            ),
        ])
        .unwrap();
    let provisional = store
        .content_descriptor(&live_content_source(&start, 0), Rep::RawJsonUtf8)
        .unwrap()
        .unwrap();
    assert_eq!(
        provisional.readable_end, 0,
        "an opened raw stream has a real empty provisional prefix"
    );
    commit_provider_outcome(&store, "1", &request);
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[frame(&start, 2, PublicationPayload::TerminalOnly)],
        )
        .unwrap();
    store
        .commit_canonical_publication(
            &start.stream_id,
            &assistant_with_tool(&start.message_id, &call),
            committed_event(&start.message_id, "1"),
        )
        .unwrap();
    let source = ContentSource::CanonicalMessageBlock {
        component: rustx::observation::content::CanonicalContentComponent::Payload,
        locator: rustx::observation::content::CanonicalContentLocator::ByMessageId {
            message_id: start.message_id,
        },
        block_index: ContentBlockIndex::new(0),
    };
    assert!(
        store
            .content_descriptor(&source, Rep::RawJsonUtf8)
            .unwrap()
            .is_none(),
        "a parsed Value cannot manufacture an original raw JSON representation"
    );
    let structural = store
        .content_descriptor(&source, Rep::CanonicalJsonV1)
        .unwrap()
        .unwrap();
    assert_eq!(read_content_bytes(&store, &structural), b"{}");
    assert!(store.content_read(&provisional, 0, 1).is_err());
}

#[test]
fn t08_t09_t19_lineage_keeps_original_raw_bytes_after_parent_removal() {
    use rustx::durable::LineageSeed;
    use rustx::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
    use rustx::observation::content::{
        CanonicalContentComponent as Component, ContentRepresentation as Rep, ContentSource,
    };
    let parent = Durable::new();
    let original = "λ🙂".repeat(350_000);
    let raw = format!(
        "{{ \n \"text\" : {} \n}}\t",
        serde_json::to_string(&original).unwrap()
    );
    let store = parent.open();
    store.initialize(&[]).unwrap();
    let (request, start) = opened_test_stream(&store, "1");
    let call = ToolCall {
        id: ToolCallId::new("lineage-raw-call"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".into(),
        arguments: serde_json::from_str(&raw).unwrap(),
    };
    store
        .stage_publication_frames(&[
            proposal_start_frame(&start, 0, &call.id),
            proposal_arguments_frame(&start, 1, ContentBlockIndex::new(0), &call.id, &raw),
            frame(
                &start,
                2,
                PublicationPayload::ProposedToolCallCompleted {
                    block_index: ContentBlockIndex::new(0),
                    call: call.clone(),
                },
            ),
        ])
        .unwrap();
    commit_provider_outcome(&store, "1", &request);
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[frame(&start, 3, PublicationPayload::TerminalOnly)],
        )
        .unwrap();
    store
        .commit_canonical_publication(
            &start.stream_id,
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: start.message_id.clone(),
                content: vec![AssistantContentBlock::ToolCall(call)],
            }),
            committed_event(&start.message_id, "1"),
        )
        .unwrap();
    let inbound = MessageId::new("inherited-large-user");
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: inbound.clone(),
            content: vec![UserContentBlock::Text(TextBlock {
                text: original.clone(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }))
        .unwrap();
    let parent_source = ContentSource::CanonicalMessageBlock {
        component: Component::Payload,
        locator: rustx::observation::content::CanonicalContentLocator::ByMessageId {
            message_id: start.message_id.clone(),
        },
        block_index: ContentBlockIndex::new(0),
    };
    let parent_ref = store
        .content_descriptor(&parent_source, Rep::RawJsonUtf8)
        .unwrap()
        .unwrap();
    let captured = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(captured.raw_tool_arguments.len(), 1);
    assert_eq!(captured.raw_tool_arguments[0].bytes, raw.as_bytes());
    let seed = LineageSeed::replayed(captured.canonical, captured.surface_history)
        .unwrap()
        .with_completed_responses(captured.completed_responses)
        .unwrap()
        .with_turns(captured.turns)
        .unwrap()
        .with_projections(captured.projections)
        .unwrap()
        .with_raw_tool_arguments(captured.raw_tool_arguments)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.sqlite");
    let child_id = ConversationId::generate();
    let child = SqliteConversationStore::open(child_id.clone(), &path).unwrap();
    child.initialize_lineage(&seed).unwrap();
    drop(store);
    drop(parent);
    drop(child);
    let child = SqliteConversationStore::open(child_id.clone(), &path).unwrap();
    assert!(child.content_read(&parent_ref, 0, 1).is_err());
    let child_ref = child
        .content_descriptor(&parent_source, Rep::RawJsonUtf8)
        .unwrap()
        .unwrap();
    assert_eq!(child_ref.conversation_id, child_id);
    assert_eq!(read_content_bytes(&child, &child_ref), raw.as_bytes());
    let user_ref = child
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                component: Component::Payload,
                locator: rustx::observation::content::CanonicalContentLocator::ByMessageId {
                    message_id: inbound,
                },
                block_index: ContentBlockIndex::new(0),
            },
            Rep::Utf8Text,
        )
        .unwrap()
        .unwrap();
    assert_eq!(read_content_bytes(&child, &user_ref), original.as_bytes());
    let heads = child
        .message_header_page(&child.observation_read_cut().unwrap(), None, 32)
        .unwrap();
    assert!(
        heads
            .headers
            .iter()
            .flat_map(|header| &header.content)
            .all(|binding| binding.content_ref.conversation_id == child_id)
    );
    assert!(
        heads
            .headers
            .iter()
            .flat_map(|header| &header.content)
            .any(|binding| binding.content_ref.representation == Rep::RawJsonUtf8)
    );
}

#[test]
fn t08_t12_t19_live_and_audit_tool_metadata_keep_exact_components_and_frame_frontiers() {
    use rustx::observation::content::{
        ContentRepresentation as Rep, ContentSource, PublicationContentComponent as Component,
    };
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (request, start) = opened_test_stream(&store, "1");
    let before_audit = store.observation_read_cut().unwrap();
    let name = "工具 name ".repeat(250_000);
    let call_id = ToolCallId::new("large-name");
    store
        .stage_publication_frames(&[proposal_start_frame_with(
            &start,
            0,
            ContentBlockIndex::new(0),
            &call_id,
            ToolId::new("tool-alpha"),
            &name,
        )])
        .unwrap();
    let mut source = live_content_source(&start, 0);
    let ContentSource::LivePublicationPrefix { component, .. } = &mut source else {
        unreachable!()
    };
    *component = Component::BlockMetadata;
    let first = store
        .content_descriptor_at(&source, Rep::CanonicalJsonV1, 1)
        .unwrap()
        .unwrap();
    assert!(first.readable_end > 1_048_576);
    assert!(
        store
            .content_descriptor_at(&source, Rep::CanonicalJsonV1, 2)
            .unwrap()
            .is_none()
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&read_content_bytes(&store, &first)).unwrap();
    assert_eq!(metadata["name"], name);
    assert_eq!(metadata["id"], call_id.as_str());
    assert!(metadata.get("arguments").is_none());
    let mut forged = first.clone();
    let ContentSource::LivePublicationPrefix { component, .. } = &mut forged.source else {
        unreachable!()
    };
    *component = Component::Payload;
    assert!(store.content_read(&forged, 0, 1).is_err());
    commit_provider_outcome(&store, "1", &request);
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[frame(&start, 1, PublicationPayload::TerminalOnly)],
        )
        .unwrap();
    store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .unwrap();
    assert!(store.content_read(&first, 0, 1).is_err());
    let source = ContentSource::DurableTranscriptAudit {
        component: Component::BlockMetadata,
        stream_id: start.stream_id,
        message_id: start.message_id,
        attempt_id: start.attempt_id,
        turn_id: start.turn_id,
        publication_request_id: start.request_id,
        block_index: ContentBlockIndex::new(0),
    };
    let retained = store
        .content_descriptor(&source, Rep::CanonicalJsonV1)
        .unwrap()
        .unwrap();
    assert!(
        store
            .validate_content_source_cut(&retained, &before_audit)
            .is_err()
    );
    store
        .validate_content_source_cut(&retained, &store.observation_read_cut().unwrap())
        .unwrap();
    drop(store);
    let metadata: serde_json::Value =
        serde_json::from_slice(&read_content_bytes(&durable.open(), &retained)).unwrap();
    assert_eq!(metadata["name"], name);
}

/// T08/T15/T20: the original `RequestStart` transaction owns one typed,
/// allowlisted inspection stream. Reopen and diagnostic corruption prove
/// a small read never reconstructs its enormous Snapshot or Surface.
#[test]
fn t08_t15_t20_request_detail_ranges_are_frozen_allowlisted_and_physically_bounded() {
    let durable = Durable::new();
    let prompt = "λ🙂".repeat(700_000);
    // Keep the aggregate canonical input above nine MiB, while every
    // original record satisfies the native model-input admission bound.
    let body = "model context ".repeat(10_000);
    let store = durable.open();
    let messages: Vec<_> = (0..73)
        .map(|index| {
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(format!("frozen-context-{index}")),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: body.clone(),
                })],
            })
        })
        .collect();
    assert!(
        messages
            .iter()
            .all(|message| serde_json::to_vec(message).unwrap().len() < 1_048_576)
    );
    assert!(messages.len() * body.len() > 9 * 1_048_576);
    store.initialize(&messages).unwrap();
    let head = store.load_head().unwrap();
    let mut configured = invocation();
    configured
        .request_params
        .insert("top_p".into(), serde_json::json!(0.9));
    configured.request_params.insert(
        "api_key".into(),
        serde_json::json!("secret-key".repeat(200_000)),
    );
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new("trace-source"),
            retry_number: 0,
        },
        rustx::context::resources::LedgerFrontier(head.revision.get()),
        prompt.clone(),
        Vec::new(),
        rustx::runtime::RuntimeResourceRevision::new(1),
        configured,
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
    crate::freeze_request(&store, &mut snapshot);
    let started = store
        .commit_model_turn_start(&[], &snapshot, fixed_time())
        .unwrap()
        .started;
    let request = snapshot.request_id;
    let header = store
        .request_summary_header(&request, started.sequence)
        .unwrap();
    assert!(header.deferred);
    assert!(serde_json::to_vec(&header).unwrap().len() < 16_384);
    assert_eq!(
        store
            .request_identity_at(started.sequence, started.sequence)
            .unwrap(),
        Some(request.clone())
    );
    assert!(
        store
            .request_summary_header(&request, started.sequence - 1)
            .is_err()
    );
    drop(store);
    let connection = rusqlite::Connection::open(&durable.path).unwrap();
    connection
        .execute(
            "UPDATE request_snapshots SET snapshot_json='invalid JSON'",
            [],
        )
        .unwrap();
    connection
        .execute("UPDATE message_ledger SET message_json='invalid JSON'", [])
        .unwrap();
    let indexed_bytes: u64 = connection
        .query_row(
            "SELECT COALESCE(SUM(length(bytes)),0) FROM content_segments WHERE source_kind='trace_request'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // Only the original Snapshot metadata plus finite C-owned descriptors
    // is indexed. The nine-megabyte canonical input is never copied into a
    // second request/display history.
    assert!(indexed_bytes < prompt.len() as u64 + 1_000_000);
    drop(connection);
    let store = durable.open();
    let prompt_path = vec!["effective_system_prompt".into(), "text".into()];
    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        let chunk = store
            .request_value_read(&request, started.sequence, &prompt_path, offset, 65_536)
            .unwrap();
        assert!(chunk.bytes.len() <= 65_536);
        bytes.extend_from_slice(&chunk.bytes);
        if let Some(next) = chunk.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), prompt);
    let mut offset = 0;
    let mut count = 0;
    loop {
        let page = store
            .request_field_page(&request, started.sequence, &["messages".into()], offset, 32)
            .unwrap();
        assert_eq!(page.total, 73);
        assert!(page.fields.len() <= 32);
        assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
        count += page.fields.len();
        if let Some(next) = page.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(count, 73);
    let option = store
        .request_value_read(&request, started.sequence, &["options".into()], 0, 65_536)
        .unwrap();
    let decoded = String::from_utf8(option.bytes).unwrap();
    assert!(decoded.contains("top_p"));
    assert!(!decoded.contains("api_key") && !decoded.contains("secret-key"));
    assert!(
        store
            .request_value_read(&request, started.sequence, &["api_key".into()], 0, 1)
            .is_err()
    );
    assert!(
        store
            .request_value_read(
                &request,
                started.sequence,
                &prompt_path,
                bytes.len() as u64 + 1,
                1
            )
            .is_err()
    );
    assert!(
        store
            .request_value_read(&request, started.sequence, &prompt_path, 0, 65_537)
            .is_err()
    );
}

#[test]
fn t08_t15_request_field_pages_advance_past_large_keys_without_losing_source_bytes() {
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let huge_key = "ζ🙂".repeat(400_000);
    let mut schema = serde_json::Map::new();
    for index in 0..80 {
        schema.insert(format!("field-{index:03}"), serde_json::json!(index));
    }
    // Reserved-looking keys also use ordinal locators, preventing collision
    // between a genuine native key and a generated bounded read locator.
    schema.insert("@80".into(), serde_json::json!("reserved-looking-key"));
    schema.insert(huge_key.clone(), serde_json::json!("large-key-value"));
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new("source-large-key"),
            retry_number: 0,
        },
        rustx::context::resources::LedgerFrontier(store.load_head().unwrap().revision.get()),
        "prompt".into(),
        Vec::new(),
        rustx::runtime::RuntimeResourceRevision::new(1),
        invocation(),
        64_000,
        None,
        false,
        vec![rustx::tools::types::ModelToolDefinition {
            id: ToolId::new("schema-tool"),
            name: "schema_tool".into(),
            description: "description".into(),
            input_schema: serde_json::Value::Object(schema),
        }],
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    );
    crate::freeze_request(&store, &mut snapshot);
    let sequence = store
        .commit_model_turn_start(&[], &snapshot, fixed_time())
        .unwrap()
        .started
        .sequence;
    let request = snapshot.request_id;
    drop(store);
    let connection = rusqlite::Connection::open(&durable.path).unwrap();
    connection
        .execute(
            "UPDATE request_snapshots SET snapshot_json='invalid JSON'",
            [],
        )
        .unwrap();
    drop(connection);
    let store = durable.open();
    let parent = vec![
        "tools".into(),
        "0".into(),
        "input_schema".into(),
        "value".into(),
    ];
    let mut offset = 0;
    let mut count = 0;
    let mut large = None;
    let mut reserved = None;
    loop {
        let page = store
            .request_field_page(&request, sequence, &parent, offset, 32)
            .unwrap();
        assert_eq!(page.total, 82);
        assert!(page.fields.len() <= 32);
        assert!(serde_json::to_vec(&page).unwrap().len() < 786_432);
        for field in &page.fields {
            assert!(serde_json::to_vec(&field.path).unwrap().len() < 4096);
            if let Some(key) = &field.key {
                if key.encoded_bytes > 65_536 {
                    large = Some(key.clone());
                    assert_eq!(field.value, serde_json::json!("large-key-value"));
                } else {
                    reserved = Some(key.clone());
                }
            }
        }
        count += page.fields.len();
        if let Some(next) = page.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(count, 82);
    let large = large.unwrap();
    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        let chunk = store
            .request_value_read(&request, sequence, &large.path, offset, 65_536)
            .unwrap();
        assert!(chunk.bytes.len() <= 65_536);
        bytes.extend_from_slice(&chunk.bytes);
        if let Some(next) = chunk.next_offset {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), huge_key);
    let reserved = reserved.unwrap();
    let bytes = store
        .request_value_read(&request, sequence, &reserved.path, 0, 65_536)
        .unwrap()
        .bytes;
    assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), "@80");
    assert!(
        store
            .request_value_read(
                &request,
                sequence,
                &[
                    "tools".into(),
                    "0".into(),
                    "input_schema".into(),
                    "value".into(),
                    huge_key
                ],
                0,
                1
            )
            .is_err()
    );
}

#[test]
fn t19_staging_receipts_pin_each_committed_frame_without_later_source_reads() {
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (request, start) = opened_test_stream(&store, "1");
    let receipts = store
        .stage_publication_frames(&[text(0, &start, "α"), text(1, &start, "🙂")])
        .unwrap();
    assert_eq!(receipts.len(), 2);
    for (index, receipt) in receipts.iter().enumerate() {
        assert_eq!(receipt.stream_id, start.stream_id);
        assert_eq!(receipt.sequence, index as u64);
        assert_eq!(receipt.content.len(), 1);
        assert_eq!(receipt.content[0].content_revision, index as u64 + 1);
    }
    assert_eq!(receipts[0].suffix_bytes, Some(2));
    assert_eq!(receipts[1].suffix_bytes, Some(4));
    assert_eq!(receipts[0].content[0].readable_end, 2);
    assert_eq!(receipts[1].content[0].readable_end, 6);
    assert_eq!(
        read_content_bytes(&store, &receipts[0].content[0]),
        "α".as_bytes()
    );
    // A refused P transaction cannot return a receipt or grant the proposed suffix.
    assert!(
        store
            .commit_publication_terminal(&start.stream_id, &[text(2, &start, "secret")])
            .is_err()
    );
    assert_eq!(
        read_content_bytes(&store, &receipts[1].content[0]),
        "α🙂".as_bytes()
    );
    commit_provider_outcome(&store, "1", &request);
    let terminal = store
        .commit_publication_terminal(&start.stream_id, &[text(2, &start, "!")])
        .unwrap();
    assert_eq!(terminal[0].content[0].readable_end, 7);
    assert_eq!(terminal[0].content[0].content_revision, 3);
    assert_eq!(terminal[0].suffix_bytes, Some(1));
    assert_eq!(
        read_content_bytes(&store, &receipts[0].content[0]),
        "α".as_bytes()
    );
}

#[test]
fn t15_t19_t20_live_native_pages_seek_sparse_block_and_tool_ordinals_at_fixed_p_frontier() {
    use rustx::observation::content::{ContentSource, PublicationContentComponent};
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (_request, start) = opened_test_stream(&store, "1");
    let mut frames = Vec::new();
    for index in 0..73_u32 {
        let block = ContentBlockIndex::new((72 - index) * 3);
        let payload = if index % 3 == 0 {
            PublicationPayload::ProposedToolCallStarted {
                block_index: block,
                call: rustx::tools::types::ToolCallStart {
                    id: ToolCallId::new(format!("sparse-live-{index}")),
                    tool_id: ToolId::new("sparse-tool"),
                    name: "sparse".into(),
                },
            }
        } else {
            PublicationPayload::TextSuffix {
                block_index: block,
                suffix: format!("text-{index}"),
            }
        };
        frames.push(PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            sequence: u64::from(index),
            payload,
        });
    }
    let receipts = store.stage_publication_frames(&frames).unwrap();
    for (index, receipt) in receipts.iter().enumerate() {
        assert_eq!(receipt.block_ordinal, Some(index as u64));
        assert_eq!(receipt.block_total, index as u64 + 1);
        assert_eq!(receipt.tool_total, (index as u64 + 3) / 3);
        assert_eq!(
            receipt.tool_ordinal,
            if index % 3 == 0 {
                Some(index as u64 / 3)
            } else {
                None
            }
        );
    }
    let source = ContentSource::LivePublicationPrefix {
        component: PublicationContentComponent::Payload,
        stream_id: start.stream_id.clone(),
        message_id: start.message_id.clone(),
        attempt_id: start.attempt_id.clone(),
        turn_id: start.turn_id.clone(),
        publication_request_id: start.request_id.clone(),
        block_index: ContentBlockIndex::new(216),
    };
    // Stage farther ahead than the observed cut, then corrupt full Frame JSON:
    // header/range selection must never deserialize those complete bodies.
    rusqlite::Connection::open(&durable.path)
        .unwrap()
        .execute(
            "UPDATE publication_frames SET frame_json='invalid JSON'",
            [],
        )
        .unwrap();
    let first = store.live_block_header_page(&source, 33, 0, 32).unwrap();
    assert_eq!(first.total, 33);
    assert_eq!(first.headers.len(), 32);
    assert_eq!(first.next_offset, Some(32));
    for (ordinal, header) in first.headers.iter().enumerate() {
        assert_eq!(header.position, ordinal as u64);
        assert_eq!(
            header.block_index.get(),
            (72 - u32::try_from(ordinal).unwrap()) * 3
        );
        assert!(
            header
                .content
                .iter()
                .all(|reference| reference.content_revision <= 33)
        );
    }
    let second = store.live_block_header_page(&source, 33, 32, 32).unwrap();
    assert_eq!(second.headers.len(), 1);
    assert!(second.next_offset.is_none());
    let tools = store.live_tool_header_page(&source, 33, 0, 32).unwrap();
    assert_eq!(tools.total, 11);
    assert_eq!(tools.headers.len(), 11);
    assert!(tools.next_offset.is_none());
    for (ordinal, header) in tools.headers.iter().enumerate() {
        assert_eq!(header.position, ordinal as u64);
        assert_eq!(header.kind, "tool_call");
        assert!(!header.assembled);
        assert_eq!(header.content.len(), 2);
        assert!(
            header
                .content
                .iter()
                .all(|reference| reference.content_revision <= 33)
        );
    }
    let mut forged = source;
    let ContentSource::LivePublicationPrefix { attempt_id, .. } = &mut forged else {
        unreachable!();
    };
    *attempt_id = AttemptId::new("different-owner");
    assert!(store.live_block_header_page(&forged, 33, 0, 32).is_err());
}

#[test]
fn t09_t19_live_metadata_revisions_keep_exact_frame_evidence() {
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (_request, start) = opened_test_stream(&store, "1");
    let block = ContentBlockIndex::new(0);
    let id = ToolCallId::new("metadata-revision");
    let tool = ToolId::new("metadata-tool");
    let first = store
        .stage_publication_frames(&[PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            sequence: 0,
            payload: PublicationPayload::ProposedToolCallStarted {
                block_index: block,
                call: rustx::tools::types::ToolCallStart {
                    id: id.clone(),
                    tool_id: tool.clone(),
                    name: "alpha".into(),
                },
            },
        }])
        .unwrap();
    let second = store
        .stage_publication_frames(&[PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            sequence: 1,
            payload: PublicationPayload::ProposedToolCallCompleted {
                block_index: block,
                call: rustx::tools::types::ToolCall {
                    id,
                    tool_id: tool,
                    name: "alpha".into(),
                    arguments: serde_json::json!({}),
                },
            },
        }])
        .unwrap();
    let old = first[0].content.last().unwrap();
    let new = second[0].content.last().unwrap();
    assert_eq!(old.readable_end, new.readable_end);
    assert_ne!(old.content_revision, new.content_revision);
    let original: serde_json::Value =
        serde_json::from_slice(&read_content_bytes(&store, old)).unwrap();
    let replaced: serde_json::Value =
        serde_json::from_slice(&read_content_bytes(&store, new)).unwrap();
    assert_eq!(original["name"], "alpha");
    assert_eq!(replaced["name"], "alpha");
    assert_eq!(original, replaced);
    drop(store);
    let store = durable.open();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&read_content_bytes(&store, old)).unwrap()["name"],
        "alpha"
    );
}

#[test]
fn t08_t12_canonical_locator_namespaces_preserve_literal_ids_and_full_inline_id_budget() {
    use rustx::observation::content::{
        CanonicalContentComponent as Component, CanonicalContentLocator as Locator,
        ContentRepresentation as Rep, ContentSource,
    };
    let durable = Durable::new();
    let store = durable.open();
    let exact = MessageId::new("x".repeat(256));
    let message = |id: MessageId, text: &str| {
        MessageBlock::User(UserMessageBlock {
            id,
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
            content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
        })
    };
    store
        .initialize(&[
            message(MessageId::new("ledger:2"), "first"),
            message(exact.clone(), "other"),
        ])
        .unwrap();
    let source = |locator| ContentSource::CanonicalMessageBlock {
        component: Component::Payload,
        locator,
        block_index: ContentBlockIndex::new(0),
    };
    let literal = store
        .content_descriptor(
            &source(Locator::ByMessageId {
                message_id: MessageId::new("ledger:2"),
            }),
            Rep::Utf8Text,
        )
        .unwrap()
        .unwrap();
    let ordinal = store
        .content_descriptor(
            &source(Locator::ByLedgerPosition { position: 2 }),
            Rep::Utf8Text,
        )
        .unwrap()
        .unwrap();
    assert_ne!(literal.source, ordinal.source);
    assert_eq!(read_content_bytes(&store, &literal), b"first");
    assert_eq!(read_content_bytes(&store, &ordinal), b"other");
    let exact = store
        .content_descriptor(
            &source(Locator::ByMessageId { message_id: exact }),
            Rep::Utf8Text,
        )
        .unwrap()
        .unwrap();
    assert_eq!(read_content_bytes(&store, &exact), b"other");
}

/// The trigger rejects only derived display bytes. Native P/U/C remain the
/// original publication authority, including exact retained raw Tool JSON.
#[test]
fn t10_t19_display_layout_failure_preserves_native_publication_and_original_raw_json() {
    use rustx::observation::content::{
        CanonicalContentComponent, CanonicalContentLocator, ContentRepresentation, ContentSource,
    };
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (request, start) = opened_test_stream(&store, "layout-failure");
    let call = ToolCallId::new("raw-layout-failure");
    let connection = rusqlite::Connection::open(&durable.path).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_display_frontier BEFORE INSERT ON content_frontiers BEGIN SELECT RAISE(ABORT,'derived display layout unavailable'); END;").unwrap();
    let first = store
        .stage_publication_frames(&[proposal_start_frame(&start, 0, &call)])
        .unwrap();
    assert!(first[0].unavailable);
    // Remove the failure before the next suffix. The missing earlier layout
    // must still fence later display appends instead of creating a false base.
    connection
        .execute_batch("DROP TRIGGER reject_display_frontier")
        .unwrap();
    let raw = " { \n } ";
    let rest = store
        .stage_publication_frames(&[
            proposal_arguments_frame(&start, 1, ContentBlockIndex::new(0), &call, raw),
            proposal_complete_frame_with(
                &start,
                2,
                ContentBlockIndex::new(0),
                &call,
                ToolId::new("tool-alpha"),
                "alpha",
            ),
        ])
        .unwrap();
    assert!(rest.iter().all(|receipt| receipt.unavailable));
    let (frames, unavailable): (u64, u64) = connection
        .query_row(
            "SELECT COUNT(*),SUM(observation_indexed=0) FROM publication_frames WHERE stream_id=?1",
            [start.stream_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((frames, unavailable), (3, 3));
    commit_provider_outcome(&store, "layout-failure", &request);
    store
        .commit_publication_terminal(
            &start.stream_id,
            &[frame(&start, 3, PublicationPayload::TerminalOnly)],
        )
        .unwrap();
    let message = assistant_with_tool(&start.message_id, &call);
    store
        .commit_canonical_publication(
            &start.stream_id,
            &message,
            committed_event(&start.message_id, "layout-failure"),
        )
        .unwrap();
    assert_eq!(store.load_canonical().unwrap(), vec![message]);
    let reference = store
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                locator: CanonicalContentLocator::ByMessageId {
                    message_id: start.message_id.clone(),
                },
                component: CanonicalContentComponent::Payload,
                block_index: ContentBlockIndex::new(0),
            },
            ContentRepresentation::RawJsonUtf8,
        )
        .unwrap()
        .unwrap();
    assert_eq!(read_content_bytes(&store, &reference), raw.as_bytes());
    drop(store);
    let reopened = durable.open();
    assert_eq!(read_content_bytes(&reopened, &reference), raw.as_bytes());
}

#[test]
fn t10_t19_derived_journal_families_fence_observation_without_refusing_native_request() {
    for trigger in [
        "CREATE TRIGGER reject_read_family BEFORE INSERT ON observation_models BEGIN SELECT RAISE(ABORT,'derived models unavailable'); END;",
        "CREATE TRIGGER reject_read_family BEFORE UPDATE OF observation_statistics ON events BEGIN SELECT RAISE(ABORT,'derived statistics unavailable'); END;",
    ] {
        let durable = Durable::new();
        let store = durable.open();
        store.initialize(&[]).unwrap();
        let connection = rusqlite::Connection::open(&durable.path).unwrap();
        connection.execute_batch(trigger).unwrap();
        let (request, start) = opened_test_stream(&store, "source-family-failure");
        let unavailable: bool = connection.query_row(
            "SELECT observation_unavailable FROM events WHERE json_extract(event_json,'$.event.type')='model_request_started'",
            [], |row| row.get(0),
        ).unwrap();
        assert!(unavailable);
        let witness = store.observation_event_witness(1, 1).unwrap().unwrap();
        assert!(witness.source_unavailable);
        assert_eq!(witness.event_kind, "model_request_started");
        // Observation rejection never rolls back the authoritative request
        // snapshot, provider outcome, staging, release or canonical commit.
        assert_eq!(
            store.load_request_snapshot(&request).unwrap().request_id,
            request
        );
        store
            .stage_publication_frames(&[text(0, &start, "semantic execution continues")])
            .unwrap();
        commit_provider_outcome(&store, "source-family-failure", &request);
        store
            .commit_publication_terminal(
                &start.stream_id,
                &[frame(&start, 1, PublicationPayload::TerminalOnly)],
            )
            .unwrap();
        let message = assistant(&start.message_id, "semantic execution continues");
        store
            .commit_canonical_publication(
                &start.stream_id,
                &message,
                committed_event(&start.message_id, "source-family-failure"),
            )
            .unwrap();
        assert_eq!(store.load_canonical().unwrap(), vec![message]);
    }
}

#[test]
fn t08_t19_native_usage_u64_frontier_and_opaque_canonical_bytes_remain_exact() {
    use rustx::model::types::{ModelUsage, UsageDetails};
    use rustx::observation::content::{
        CanonicalContentComponent, CanonicalContentLocator, ContentRepresentation, ContentSource,
    };
    let store = SqliteConversationStore::in_memory(conversation_id()).unwrap();
    store.initialize(&[]).unwrap();
    let request = start_request(&store, "wide-usage");
    let event = store
        .append_event(envelope(
            "wide-usage",
            "wide-usage",
            RuntimeEvent::ModelRequestCompleted {
                request_id: request,
                finish_reason: ModelFinishReason::Stop,
                usage: Some(ModelUsage {
                    input_tokens: u64::MAX,
                    output_tokens: u64::MAX,
                    total_tokens: u64::MAX,
                    details: Some(UsageDetails {
                        reasoning_tokens: Some(u64::MAX),
                        cached_input_tokens: Some(u64::MAX),
                    }),
                }),
                generation: None,
            },
        ))
        .unwrap();
    let witness = store
        .observation_event_witness(event.sequence, event.sequence)
        .unwrap()
        .unwrap();
    assert!(!witness.source_unavailable);
    assert_eq!(
        witness.value["usage"]["output_tokens"],
        u64::MAX.to_string()
    );
    let chunk = store
        .event_value_read(
            event.sequence,
            event.sequence,
            &["event".into(), "usage".into(), "output_tokens".into()],
            0,
            65_536,
        )
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<String>(&chunk.bytes).unwrap(),
        u64::MAX.to_string()
    );
    let message = assistant_with_tools(
        &MessageId::new("wide-canonical"),
        vec![ToolCall {
            id: ToolCallId::new("wide-call"),
            tool_id: ToolId::new("wide-tool"),
            name: "wide".into(),
            arguments: serde_json::json!({"reported":u64::MAX}),
        }],
    );
    store.append_canonical(&message).unwrap();
    let reference = store
        .content_descriptor(
            &ContentSource::CanonicalMessageBlock {
                locator: CanonicalContentLocator::ByMessageId {
                    message_id: message.id().clone(),
                },
                component: CanonicalContentComponent::Payload,
                block_index: ContentBlockIndex::new(0),
            },
            ContentRepresentation::CanonicalJsonV1,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        read_content_bytes(&store, &reference),
        format!("{{\"reported\":{}}}", u64::MAX).as_bytes()
    );
}

/// Audit-only indexed seeking retains the same positive head even when the
/// newest mixed transcript page contains no audits and original bodies are unreadable.
#[test]
fn t14_t19_t20_audit_settlement_head_uses_exact_native_positions_without_body_reads() {
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let mut receipts = Vec::new();
    for index in 0..33 {
        let (_, start) = opened_test_stream(&store, &format!("head-{index}"));
        let suffix = if index == 32 {
            "🙂水".repeat(200_000)
        } else {
            format!("body-{index}")
        };
        store
            .stage_publication_frames(&[text(0, &start, &suffix)])
            .unwrap();
        let (_, receipt) = store
            .terminalize_publication_audit(&start.stream_id, fixed_time())
            .unwrap();
        let record = receipt.header.publication_settlement.as_ref().unwrap();
        assert_eq!(
            record.transcript_cursor.get(),
            receipt.transcript_cursor.get()
        );
        assert_eq!(record.read_cut, receipt.read_cut);
        assert_eq!(record.settlement.block_count, 1);
        assert_eq!(record.settlement.kind, PublicationAuditKind::Incomplete);
        assert!(serde_json::to_vec(&receipt.header).unwrap().len() <= 16_384);
        receipts.push(receipt);
    }
    let audit_cut = store.observation_read_cut().unwrap();
    assert_eq!(
        store
            .observation_trace_total(audit_cut.read.journal)
            .unwrap(),
        33
    );
    let messages = (0..73)
        .map(|index| {
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new(format!("after-audit-{index}")),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "later".into(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })
        })
        .collect::<Vec<_>>();
    store.append_canonical_batch(&messages).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let generic = store.transcript_header_page(&cut, None, 8).unwrap();
    assert!(
        generic
            .headers
            .iter()
            .all(|header| header.publication_settlement.is_none())
    );
    let expected = store.publication_settlement_head(&cut).unwrap();
    drop(store);
    let store = durable.open();
    assert_eq!(store.publication_settlement_head(&cut).unwrap(), expected);
    let connection = rusqlite::Connection::open(&durable.path).unwrap();
    connection
        .execute(
            "UPDATE publication_audits SET audit_json='unreadable original body'",
            [],
        )
        .unwrap();
    let head = store.publication_settlement_head(&cut).unwrap();
    assert_eq!(head.len(), 32);
    assert_eq!(
        head.first().unwrap(),
        receipts[1].header.publication_settlement.as_ref().unwrap()
    );
    assert_eq!(
        head.last().unwrap(),
        receipts[32].header.publication_settlement.as_ref().unwrap()
    );
    assert_eq!(store.publication_settlement_head(&audit_cut).unwrap(), head);
    connection.execute("DELETE FROM observation_headers WHERE reference_kind='publication_audit' AND reference_id=?1",[receipts[32].summary.stream_id.as_ref().unwrap().as_str()]).unwrap();
    assert!(
        store.publication_settlement_head(&cut).is_err(),
        "missing layout is unavailable, never a smaller authoritative head"
    );
}

#[test]
fn t10_t19_audit_layout_failure_retains_positive_native_settlement_receipt() {
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (_, start) = opened_test_stream(&store, "audit-layout-failure");
    store
        .stage_publication_frames(&[text(0, &start, "retained released bytes")])
        .unwrap();
    let connection = rusqlite::Connection::open(&durable.path).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_audit_header BEFORE INSERT ON observation_headers BEGIN SELECT RAISE(ABORT,'display header unavailable'); END;").unwrap();
    let (audit, receipt) = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .unwrap();
    assert_eq!(audit.kind, PublicationAuditKind::Incomplete);
    assert_eq!(receipt.summary.kind, audit.kind);
    assert_eq!(receipt.summary.block_count, 1);
    assert!(receipt.header.deferred && receipt.header.display.is_none());
    let record = receipt.header.publication_settlement.unwrap();
    assert_eq!(
        record.transcript_cursor.get(),
        receipt.transcript_cursor.get()
    );
    assert_eq!(record.read_cut, receipt.read_cut);
    assert_eq!(record.settlement.stream_id.as_ref(), Some(&start.stream_id));
    assert_eq!(
        store.load_publication_audit(&start.stream_id).unwrap(),
        Some(audit)
    );
    assert!(
        store
            .publication_settlement_head(&receipt.read_cut)
            .is_err(),
        "absent derived source is unavailable, not absence of native settlement"
    );
}

#[test]
fn t08_t14_t15_t19_audit_blocks_advance_past_giant_proposal_metadata_and_exact_raw_json() {
    use base64::Engine as _;
    use rustx::observation::content::{
        ContentRepresentation, ContentSource, PublicationContentComponent,
    };
    let durable = Durable::new();
    let store = durable.open();
    store.initialize(&[]).unwrap();
    let (_, start) = opened_test_stream(&store, "many-audit-blocks");
    let raw = " { \"wide\": 9007199254740993, \"duplicate\":1,\"duplicate\":2 } ";
    let call = ToolCallId::new("giant-audited-proposal");
    let mut frames = vec![
        frame(
            &start,
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.clone(),
                    tool_id: ToolId::new("tool".repeat(70_000)),
                    name: "name".repeat(70_000),
                },
            },
        ),
        frame(
            &start,
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call,
                suffix: raw.into(),
            },
        ),
    ];
    for index in 1..73_u32 {
        frames.push(frame(
            &start,
            u64::from(index) + 1,
            PublicationPayload::TextSuffix {
                block_index: ContentBlockIndex::new(index),
                suffix: format!("block-{index}"),
            },
        ));
    }
    store.stage_publication_frames(&frames).unwrap();
    let (_, receipt) = store
        .terminalize_publication_audit(&start.stream_id, fixed_time())
        .unwrap();
    assert!(receipt.header.deferred);
    assert_eq!(receipt.summary.block_count, 73);
    let first = store
        .audit_block_header_page(&receipt.read_cut, receipt.transcript_cursor.get(), 0, 32)
        .unwrap();
    assert_eq!(
        (first.headers.len(), first.total, first.next_offset),
        (32, 73, Some(32))
    );
    assert!(first.headers[0].deferred && first.headers[0].value.is_none());
    assert!(
        first.headers[1..]
            .iter()
            .all(|header| header.value.is_some())
    );
    assert!(
        first
            .headers
            .iter()
            .all(|header| serde_json::to_vec(header).unwrap().len() <= 16_384)
    );
    let payload = first.headers[0]
        .content
        .iter()
        .find(|binding| binding.content_ref.representation == ContentRepresentation::RawJsonUtf8)
        .unwrap();
    assert_eq!(
        read_content_bytes(&store, &payload.content_ref),
        raw.as_bytes()
    );
    let metadata = first.headers[0]
        .content
        .iter()
        .find(|binding| {
            matches!(
                binding.content_ref.source,
                ContentSource::DurableTranscriptAudit {
                    component: PublicationContentComponent::BlockMetadata,
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(
                store
                    .content_read(&metadata.content_ref, 1, 65_536)
                    .unwrap()
                    .data_base64
            )
            .unwrap()
            .len(),
        65_536
    );
    let middle = store
        .audit_block_header_page(&receipt.read_cut, receipt.transcript_cursor.get(), 32, 32)
        .unwrap();
    assert_eq!((middle.headers.len(), middle.next_offset), (32, Some(64)));
    let last = store
        .audit_block_header_page(&receipt.read_cut, receipt.transcript_cursor.get(), 64, 32)
        .unwrap();
    assert_eq!((last.headers.len(), last.next_offset), (9, None));
    assert!(
        store
            .audit_block_header_page(&receipt.read_cut, receipt.transcript_cursor.get(), 74, 32)
            .is_err()
    );
}
