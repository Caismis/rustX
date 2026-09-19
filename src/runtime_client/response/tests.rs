use super::*;
use crate::context::assembly::ContextGeneration;
use crate::durable::SqliteConversationStore;
use crate::events::types::RuntimeEventEnvelope;
use crate::message::TextBlock;
use crate::message::types::{
    AssistantMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::invocation::{ModelInvocationConfig, RequestParams};
use crate::model::{
    ModelCapabilities, ModelCompat, ModelProtocol, RequestIdentity, RequestSnapshot,
};
use crate::runtime::identity::{CapabilityRevision, ConversationId, EventId, TurnId};
use chrono::{TimeZone, Utc};

fn event(
    store: &dyn ConversationStore,
    attempt: &str,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: 1,
        event_id: EventId::new(format!(
            "event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: Some(AttemptId::new(attempt)),
        turn_id: Some(TurnId::new("1")),
        timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        event,
    }
}
fn append(store: &dyn ConversationStore, attempt: &str, kind: RuntimeEvent) {
    store.append_event(event(store, attempt, kind)).unwrap();
}
fn user(store: &dyn ConversationStore, id: &str) {
    store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            source: UserSource::Human,
            timestamp: None,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock { text: id.into() })],
        }))
        .unwrap();
}
fn assistant(store: &dyn ConversationStore, attempt: &str, id: &str) {
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new(id),
                content: vec![AssistantContentBlock::Text(TextBlock { text: id.into() })],
            }),
            event(
                store,
                attempt,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new(id),
                },
            ),
        )
        .unwrap();
}
fn request(store: &dyn ConversationStore, attempt: &str, retry: u32, usage: Option<ModelUsage>) {
    let snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new(attempt),
            turn: TurnId::new("1"),
            retry_number: retry,
        },
        store.load_head().unwrap().revision,
        "system".into(),
        vec![],
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: "historical-model".into(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 100,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        4096,
        None,
        false,
        vec![],
        CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: vec![],
        },
        None,
        vec![],
    );
    store
        .commit_model_turn_start(&[], &snapshot, Utc.timestamp_opt(1_700_000_000, 0).unwrap())
        .unwrap();
    append(
        store,
        attempt,
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id,
            finish_reason: ModelFinishReason::Stop,
            usage,
        },
    );
}
fn usage(cached: Option<u64>) -> ModelUsage {
    ModelUsage {
        input_tokens: 100,
        output_tokens: 20,
        total_tokens: 120,
        details: Some(UsageDetails {
            reasoning_tokens: None,
            cached_input_tokens: cached,
        }),
    }
}
fn page(
    store: &dyn ConversationStore,
    before: Option<crate::durable::TranscriptCursor>,
    limit: usize,
) -> RuntimeClientTranscriptPage {
    let mut page = super::super::snapshot::transcript_page_view(
        store.load_transcript_page(before, limit).unwrap(),
    )
    .unwrap();
    decorate(store, &mut page).unwrap();
    page
}
fn tails(page: &RuntimeClientTranscriptPage) -> Vec<&CompletedResponseView> {
    page.entries
        .iter()
        .filter_map(|e| e.completed_response.as_ref())
        .collect()
}
fn finish(store: &dyn ConversationStore, attempt: &str) {
    append(
        store,
        attempt,
        RuntimeEvent::AttemptCompleted {
            attempt_id: AttemptId::new(attempt),
            finish_reason: ModelFinishReason::Stop,
        },
    );
}

#[test]
fn one_attempt_many_requests_has_one_exact_tail_and_missing_buckets_stay_absent() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    user(&store, "input-a");
    append(
        &store,
        "a",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("a"),
        },
    );
    request(&store, "a", 0, Some(usage(Some(0))));
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("intermediate"),
                content: vec![AssistantContentBlock::ToolCall(
                    crate::tools::types::ToolCall {
                        id: crate::runtime::identity::ToolCallId::new("call-a"),
                        tool_id: crate::runtime::identity::ToolId::new("tool-a"),
                        name: "lookup".into(),
                        arguments: serde_json::json!({}),
                    },
                )],
            }),
            event(
                &store,
                "a",
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("intermediate"),
                },
            ),
        )
        .unwrap();
    request(&store, "a", 1, Some(usage(None)));
    assistant(&store, "a", "closing");
    assert!(tails(&page(&store, None, 64)).is_empty());
    assert!(!is_completed_response(&store, &MessageId::new("closing")).unwrap());
    finish(&store, "a");
    let result = page(&store, None, 64);
    let tails = tails(&result);
    assert_eq!(tails.len(), 1);
    assert_eq!(tails[0].closing_message_id, MessageId::new("closing"));
    assert_eq!(tails[0].retry_message_id, Some(MessageId::new("input-a")));
    assert_eq!(tails[0].usage.as_ref().unwrap().total_tokens, 240);
    assert_eq!(
        tails[0]
            .usage
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .cached_input_tokens,
        None
    );
    assert!(is_completed_response(&store, &MessageId::new("closing")).unwrap());
    assert!(!is_completed_response(&store, &MessageId::new("intermediate")).unwrap());
    assert!(
        !serde_json::to_value(tails[0])
            .unwrap()
            .to_string()
            .contains("timing")
    );
}

#[test]
fn historical_pages_reopen_and_later_attempts_preserve_identity_and_totals() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let id = ConversationId::new("conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    for attempt in ["a", "b"] {
        user(&store, &format!("input-{attempt}"));
        append(
            &store,
            attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(attempt),
            },
        );
        request(&store, attempt, 0, Some(usage(Some(80))));
        assistant(&store, attempt, &format!("response-{attempt}"));
        finish(&store, attempt);
    }
    let newest = page(&store, None, 1);
    let older = page(&store, newest.next_cursor.map(Into::into), 64);
    assert_eq!(tails(&newest)[0].attempt_id, AttemptId::new("b"));
    assert_eq!(tails(&older)[0].attempt_id, AttemptId::new("a"));
    assert_eq!(newest.statistics, older.statistics);
    assert_eq!(
        newest
            .statistics
            .as_ref()
            .unwrap()
            .reported_usage
            .as_ref()
            .unwrap()
            .total_tokens,
        240
    );
    drop(store);
    let reopened = SqliteConversationStore::open_existing(id, &path).unwrap();
    assert_eq!(newest, page(&reopened, None, 1));
    assert_eq!(
        older,
        page(&reopened, newest.next_cursor.map(Into::into), 64)
    );
}

#[test]
fn published_read_cut_does_not_observe_a_later_completion() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    append(
        &store,
        "a",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("a"),
        },
    );
    assistant(&store, "a", "closing");
    let through = store.presentation_frontier().unwrap();
    finish(&store, "a");
    let mut old = page(&store, None, 64);
    for entry in &mut old.entries {
        entry.completed_response = None;
    }
    decorate_through(&store, &mut old, through).unwrap();
    assert!(tails(&old).is_empty());
    assert_eq!(tails(&page(&store, None, 64)).len(), 1);
}

#[test]
fn interrupted_output_and_incomplete_usage_never_claim_completion_or_zero() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    append(
        &store,
        "a",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("a"),
        },
    );
    request(&store, "a", 0, None);
    assistant(&store, "a", "partial");
    append(
        &store,
        "a",
        RuntimeEvent::AttemptTimedOut {
            attempt_id: AttemptId::new("a"),
        },
    );
    assert!(tails(&page(&store, None, 64)).is_empty());
    append(
        &store,
        "b",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("b"),
        },
    );
    request(&store, "b", 0, Some(usage(Some(0))));
    request(&store, "b", 1, None);
    assistant(&store, "b", "final");
    finish(&store, "b");
    let result = page(&store, None, 64);
    assert_eq!(tails(&result)[0].usage, None);
    assert_eq!(result.statistics.as_ref().unwrap().requests_with_usage, 1);
    assert_eq!(result.statistics.as_ref().unwrap().model_requests, 3);
}

#[test]
fn context_measurement_is_native_and_is_invalidated_by_compaction() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    append(
        &store,
        "a",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("a"),
        },
    );
    request(&store, "a", 0, Some(usage(Some(80))));
    let read = crate::context::occupancy::read(&store, store.presentation_frontier().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(read.input_tokens, 100);
    assert_eq!(read.context_window_tokens, 4096);
    assert_eq!(read.model, "historical-model");
    let totals = page(&store, None, 64).statistics;
    append(&store, "a", RuntimeEvent::CompactionStarted);
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        None
    );
    assert_eq!(page(&store, None, 64).statistics, totals);
}

#[test]
fn real_compaction_preserves_response_identity_cut_and_cumulative_usage() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    user(&store, "input-a");
    append(
        &store,
        "a",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("a"),
        },
    );
    request(&store, "a", 0, Some(usage(Some(80))));
    assistant(&store, "a", "response-a");
    finish(&store, "a");
    let before = page(&store, None, 64);
    let tail = tails(&before)[0].clone();
    store
        .commit_compaction(crate::durable::CompactionCommitInput {
            summary: UserMessageBlock {
                id: MessageId::new("summary"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "summary".into(),
                })],
                source: UserSource::Runtime,
                kind: InboundKind::CompactionSummary(
                    crate::message::types::CompactionSummaryMetadata::empty(),
                ),
                timestamp: None,
            },
            span: crate::conversation::SurfaceSpan::new(
                MessageId::new("input-a"),
                MessageId::new("response-a"),
            ),
            expected_revision: store.load_head().unwrap().revision,
            tokens_before: crate::runtime::types::TokenMeasurement {
                input_tokens: 100,
                source: crate::runtime::types::TokenMeasurementSource::ProviderReported,
            },
            estimated_tokens_after: 20,
            attempt_id: None,
            turn_id: None,
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        })
        .unwrap();
    let after = page(&store, None, 64);
    assert_eq!(tails(&after)[0], &tail);
    assert_eq!(after.statistics, before.statistics);
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        None
    );
    assert!(is_completed_response(&store, &tail.closing_message_id).unwrap());
    assert_eq!(
        store
            .message_append_revision(&tail.closing_message_id)
            .unwrap(),
        Some(tail.surface_revision)
    );
}
