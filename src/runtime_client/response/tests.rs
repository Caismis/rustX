use super::*;
use crate::context::assembly::ContextGeneration;
use crate::durable::SqliteConversationStore;
use crate::durable::{CanonicalStore, JournalStore, ObservationStore, ProjectionStore};
use crate::events::types::RuntimeEventEnvelope;
use crate::message::TextBlock;
use crate::message::types::InboundKind;
use crate::message::types::{
    AssistantMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::invocation::{ModelInvocationConfig, RequestParams};
use crate::model::{
    ModelCapabilities, ModelCompat, ModelProtocol, RequestIdentity, RequestSnapshot, UsageDetails,
};
use crate::runtime::identity::{CapabilityRevision, ConversationId, EventId, TurnId};
use chrono::{TimeZone, Utc};

fn event(
    store: &dyn ConversationStore,
    attempt: &str,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
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
fn admitted_user(store: &dyn ConversationStore, id: &str, attempt: &str) {
    append(
        store,
        attempt,
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new(attempt),
        },
    );
    let accepted = store
        .accept_inbound(crate::durable::InboundDraft {
            message_id: Some(MessageId::new(id)),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![UserContentBlock::Text(TextBlock { text: id.into() })],
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            correlation: None,
        })
        .unwrap();
    store
        .adopt_pending_batch(accepted.sequence, Some(AttemptId::new(attempt)))
        .unwrap();
}
fn canonical_result(store: &dyn ConversationStore, attempt: &str, owner: &str, failed: bool) {
    let MessageBlock::Assistant(assistant) = store
        .load_messages(&[MessageId::new(owner)])
        .unwrap()
        .pop()
        .unwrap()
    else {
        panic!("physical Assistant")
    };
    let AssistantContentBlock::ToolCall(call) = &assistant.content[0] else {
        panic!("physical call")
    };
    let id = MessageId::new(format!("{owner}-result"));
    store
        .append_canonical_with_event(
            &MessageBlock::Tool(crate::message::types::ToolMessageBlock {
                id: id.clone(),
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    assistant.id,
                    crate::message::types::ContentBlockIndex::new(0),
                ),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                result: crate::tools::types::ToolExecutionResult {
                    status: if failed {
                        crate::tools::types::ToolExecutionStatus::Failed {
                            error: "controlled".into(),
                        }
                    } else {
                        crate::tools::types::ToolExecutionStatus::Success
                    },
                    content: vec![],
                    duration_ms: if failed { 1500 } else { 0 },
                    exit_code: None,
                    artifacts: vec![],
                    deliveries: vec![],
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            }),
            event(
                store,
                attempt,
                RuntimeEvent::ToolMessageCommitted {
                    message_id: id,
                    tool_call_id: call.id.clone(),
                },
            ),
        )
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
pub(crate) fn request(
    store: &dyn ConversationStore,
    attempt: &str,
    retry: u32,
    usage: Option<ModelUsage>,
) {
    request_timed(
        store,
        attempt,
        retry,
        usage,
        Some(sample_generation()),
        false,
    );
}
fn sample_generation() -> crate::model::generation_evidence::GenerationEvidence {
    crate::model::generation_evidence::GenerationEvidence {
        dispatch_after_start_ms: Some(400),
        first_output_ms: Some(320),
        last_output_ms: Some(900),
        terminal_ms: 1600,
    }
}
fn request_timed(
    store: &dyn ConversationStore,
    attempt: &str,
    retry: u32,
    usage: Option<ModelUsage>,
    generation: Option<crate::model::generation_evidence::GenerationEvidence>,
    failed: bool,
) {
    let snapshot = start_request(store, attempt, retry);
    append(
        store,
        attempt,
        if failed {
            RuntimeEvent::ModelRequestFailed {
                request_id: snapshot.request_id,
                error: crate::model::error::ModelError {
                    kind: crate::model::error::ModelErrorKind::Transport,
                    message: "controlled failure".into(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                    retry_after_ms: None,
                    provider_code: None,
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
                usage,
                generation,
            }
        } else {
            RuntimeEvent::ModelRequestCompleted {
                request_id: snapshot.request_id,
                finish_reason: ModelFinishReason::Stop,
                usage,
                generation,
            }
        },
    );
}
fn start_request(store: &dyn ConversationStore, attempt: &str, retry: u32) -> RequestSnapshot {
    start_request_with_window(store, attempt, retry, 4096)
}
fn start_request_with_window(
    store: &dyn ConversationStore,
    attempt: &str,
    retry: u32,
    context_window: u64,
) -> RequestSnapshot {
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new(attempt),
            turn: TurnId::new("1"),
            retry_number: retry,
        },
        crate::context::resources::LedgerFrontier((store.load_head().unwrap().revision).get()),
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
        context_window,
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
    let history = store.projected_history().unwrap();
    snapshot.history_cut = history.cut;
    snapshot
        .freeze_input(
            store.conversation_id(),
            &crate::model::ModelRequest {
                messages: history.messages.clone(),
                images: crate::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
                continuation_source: snapshot.continuation_source.clone(),
            },
        )
        .unwrap();
    store
        .commit_model_turn_start(&[], &snapshot, Utc.timestamp_opt(1_700_000_000, 0).unwrap())
        .unwrap();
    snapshot
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
    native_oracle_decorate(store, &mut page).unwrap();
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
fn conversation_totals_and_clock_are_native_and_independent_of_loaded_rows() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    for attempt in ["a", "b"] {
        user(&store, &format!("input-{attempt}"));
        append(
            &store,
            attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(attempt),
            },
        );
        append(&store, attempt, RuntimeEvent::TurnStarted);
        request(&store, attempt, 0, Some(usage(Some(20))));
        let call = crate::runtime::identity::ToolCallId::new("call");
        let tool_id = crate::runtime::identity::ToolId::new("tool-bash");
        let proposal = MessageId::new(format!("proposal-{attempt}"));
        store
            .append_canonical_with_event(
                &MessageBlock::Assistant(AssistantMessageBlock {
                    id: proposal.clone(),
                    content: vec![AssistantContentBlock::ToolCall(crate::tools::ToolCall {
                        id: call.clone(),
                        tool_id: tool_id.clone(),
                        name: "bash".into(),
                        arguments: serde_json::json!({}),
                    })],
                }),
                event(
                    &store,
                    attempt,
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: proposal,
                    },
                ),
            )
            .unwrap();
        append(
            &store,
            attempt,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.clone(),
                tool_id: tool_id.clone(),
            },
        );
        // Calls pair within their Attempt; each Attempt reuses the provider call id.
        let mut settled = event(
            &store,
            attempt,
            RuntimeEvent::ToolExecutionFailed {
                tool_call_id: call,
                tool_id,
                error: "controlled".into(),
            },
        );
        settled.timestamp += chrono::Duration::milliseconds(1500);
        store.append_event(settled).unwrap();
        canonical_result(&store, attempt, &format!("proposal-{attempt}"), true);
        assistant(&store, attempt, &format!("answer-{attempt}"));
        finish(&store, attempt);
    }
    let full = page(&store, None, 64);
    let latest = page(&store, None, 1);
    let earlier = page(&store, latest.next_cursor.map(Into::into), 1);
    assert_eq!(full.statistics, latest.statistics);
    assert_eq!(full.statistics, earlier.statistics);
    let totals = full.statistics.clone().unwrap();
    assert_eq!(totals.turns, 2);
    assert_eq!(totals.steps, 2);
    assert_eq!(totals.model_requests, 2);
    assert_eq!(totals.reported_usage.unwrap().total_tokens, 240);
    assert_eq!(
        totals.timing.unwrap(),
        crate::durable::response::ConversationTiming {
            model_ms: Some(3200),
            tool_ms: Some(3000),
            mean_ttft_ms: Some(320),
            output_tokens_per_second: Some(15.625),
        }
    );
    assert_eq!(tails(&full)[0].models, ["historical-model"]);
    let clock = totals.latest_turn.unwrap();
    assert_eq!(clock.attempt_id, Some(AttemptId::new("b")));
    assert!(!clock.attempt_id_deferred);
    assert!(clock.attempt_id_source.is_none());
    assert_eq!(clock.ended_at, Some(clock.started_at));
}

#[test]
fn one_attempt_many_requests_has_one_exact_tail_and_missing_buckets_stay_absent() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    admitted_user(&store, "input-a", "a");

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
    canonical_result(&store, "a", "intermediate", false);
    request(&store, "a", 1, Some(usage(None)));
    assistant(&store, "a", "closing");
    assert!(tails(&page(&store, None, 64)).is_empty());
    assert!(!is_completed_response(&store, &MessageId::new("closing")).unwrap());
    finish(&store, "a");
    let result = page(&store, None, 64);
    let tails = tails(&result);
    assert_eq!(tails.len(), 1);
    assert_eq!(tails[0].closing_message_id, Some(MessageId::new("closing")));
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
    let timing = tails[0].timing.as_ref().unwrap();
    assert_eq!(timing.total_duration_ms, Some(0));
    assert_eq!(timing.ttft_ms, Some(320));
    assert_eq!(timing.generation_ms, Some(2560));
    assert_eq!(timing.output_tokens_per_second, Some(15.625));
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
    assert_eq!(
        tails(&newest)[0].origin.attempt_id,
        Some(AttemptId::new("b"))
    );
    assert_eq!(
        tails(&older)[0].origin.attempt_id,
        Some(AttemptId::new("a"))
    );
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
    native_oracle_decorate_through(&store, &mut old, through).unwrap();
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
fn context_measurement_survives_pending_and_failed_compaction() {
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
    // The `"system"` prompt prices at ceil(8 / 4); messages are the measured remainder.
    assert_eq!(
        read.breakdown,
        crate::context::occupancy::ContextBreakdown {
            system_tokens: 2,
            tool_tokens: 0,
            message_tokens: 98
        }
    );
    // Streaming has no new provider sample yet, including after reopening a read.
    start_request(&store, "a", 1);
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        Some(read.clone())
    );
    // Usage-free terminals must not erase the measurement, even across pages.
    for retry in 2..=2049 {
        request(&store, "a", retry, None);
    }
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        Some(read.clone()),
        "a newer unmeasured request preserves the last measured reading"
    );
    let mut zero = usage(None);
    zero.input_tokens = 0;
    request(&store, "a", 2050, Some(zero));
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap())
            .unwrap()
            .unwrap()
            .input_tokens,
        0,
        "a genuine measured zero remains available"
    );
    let totals = page(&store, None, 64).statistics;
    append(
        &store,
        "a",
        RuntimeEvent::CompactionStarted {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
        },
    );
    let pending = crate::context::occupancy::read(&store, store.presentation_frontier().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        pending.input_tokens, 0,
        "maintenance preserves even a measured zero"
    );
    assert!(!pending.estimated);
    append(
        &store,
        "a",
        RuntimeEvent::CompactionFailed {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
            error: "summary failed".into(),
        },
    );
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        Some(pending)
    );
    assert_eq!(page(&store, None, 64).statistics, totals);
}

#[test]
#[allow(clippy::too_many_lines)]
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
    let outline_before = store.conversation_turns(0, 64).unwrap();
    let before = page(&store, None, 64);
    let tail = tails(&before)[0].clone();
    let mut input = crate::scripted_suites::support::context::checkpoint_commit_fixture(
        &store,
        crate::scripted_suites::support::context::checkpoint_fixture(
            &store, "summary", 2, "summary",
        ),
        None,
    );
    input.occupancy = Some(crate::context::occupancy::estimate(
        20,
        4096,
        "compaction-primary",
        "system",
        &[],
    ));
    store.commit_projection(input).unwrap();
    let outline_after = store.conversation_turns(0, 64).unwrap();
    assert_eq!(outline_before.turns, outline_after.turns);
    assert_ne!(outline_before.cut, outline_after.cut);
    let after = page(&store, None, 64);
    assert_eq!(
        after.entries.len(),
        before.entries.len(),
        "projection commits no transcript row"
    );
    let checkpoint = store.projection_head().unwrap().unwrap();
    assert_eq!(checkpoint.candidate.checkpoint_id.as_str(), "summary");
    assert!(checkpoint.activation.destination_installation.is_none());
    assert_eq!(tails(&after)[0], &tail);
    assert_eq!(after.statistics, before.statistics);
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        Some(crate::context::occupancy::estimate(
            20,
            4096,
            "compaction-primary",
            "system",
            &[]
        ))
    );
    assert!(
        is_completed_response(
            &store,
            tail.closing_message_id
                .as_ref()
                .expect("bounded fixture identity")
        )
        .unwrap()
    );
    assert_eq!(
        store
            .message_append_revision(
                tail.closing_message_id
                    .as_ref()
                    .expect("bounded fixture identity")
            )
            .unwrap(),
        Some(tail.surface_revision)
    );
    let child_id = ConversationId::generate();
    let cut = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    let seed = crate::local_runtime::session::lineage_cut(
        &child_id,
        &cut,
        None,
        crate::local_runtime::session::LineageSide::After,
    )
    .unwrap();
    let child = SqliteConversationStore::in_memory(child_id).unwrap();
    child.initialize_lineage(&seed).unwrap();
    let inherited = page(&child, None, 64);
    assert_eq!(
        inherited.entries.len(),
        before.entries.len(),
        "copied execution facts preserve canonical transcript cardinality"
    );
    assert!(
        child.read_events(None, 64).unwrap().events.is_empty(),
        "lineage installation creates no destination execution event"
    );
    assert_eq!(tails(&inherited).len(), 1);
    let inherited_tail = tails(&inherited)[0];
    assert_eq!(inherited_tail.origin, tail.origin);
    assert_eq!(inherited_tail.usage, tail.usage);
    assert!(
        is_completed_response(
            &child,
            inherited_tail
                .closing_message_id
                .as_ref()
                .expect("bounded fixture identity")
        )
        .unwrap()
    );
    // Totals travel with the inherited turn; its pre-compaction context
    // reading does not, because the retained Surface compacted after it.
    assert_eq!(
        inherited.statistics,
        Some(ConversationStatistics {
            latest_turn: None,
            ..after.statistics.clone().unwrap()
        })
    );
    assert_eq!(
        crate::context::occupancy::read(&child, child.presentation_frontier().unwrap()).unwrap(),
        checkpoint.occupancy
    );
}

mod lineage;
mod retry;

fn is_completed_response(
    store: &dyn ConversationStore,
    message: &MessageId,
) -> Result<bool, ConversationStoreError> {
    Ok(store
        .read_lineage_cut(store.load_head()?.revision)?
        .completed_responses
        .iter()
        .any(|response| response.closing_message_id == *message))
}

mod timing;

#[test]
fn process_membership_uses_exact_attempts_across_steering_and_pages() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f",
    ))
    .unwrap();
    user(&store, "input");
    assistant(&store, "a", "a-intermediate");
    user(&store, "steering");
    assistant(&store, "b", "b-live");
    assistant(&store, "a", "a-final");
    let live = page(&store, None, 64);
    assert!(live.entries.iter().all(|entry| {
        entry
            .turn_process
            .as_ref()
            .is_none_or(|process| process.outcome == TurnProcessOutcome::Running)
    }));
    finish(&store, "a");
    let completed = page(&store, None, 64);
    let final_position = store
        .message_ledger_position(&MessageId::new("a-final"))
        .unwrap()
        .unwrap();
    for entry in &completed.entries {
        let RuntimeClientTranscriptItem::Message { message } = &entry.item else {
            continue;
        };
        match message.id().as_str() {
            "a-intermediate" | "a-final" => {
                let process = entry.turn_process.as_ref().unwrap();
                assert_eq!(process.attempt_id, Some(AttemptId::new("a")));
                assert!(process.attempt_id_source.is_none());
                assert_eq!(process.final_message_id, Some(MessageId::new("a-final")));
                assert_eq!(
                    process.final_message_locator,
                    Some(
                        super::super::content::CanonicalContentLocator::ByLedgerPosition {
                            position: final_position,
                        }
                    )
                );
                assert!(!process.final_message_id_deferred);
                assert!(process.final_message_id_source.is_none());
            }
            _ => assert!(
                entry
                    .turn_process
                    .as_ref()
                    .is_none_or(|process| process.outcome == TurnProcessOutcome::Running)
            ),
        }
    }
    let last = page(&store, None, 1);
    let earlier = page(
        &store,
        last.next_cursor
            .map(|cursor| crate::durable::TranscriptCursor::new(cursor.get())),
        64,
    );
    let member = earlier.entries.iter().find(|entry| matches!(&entry.item,
        RuntimeClientTranscriptItem::Message { message } if message.id().as_str() == "a-intermediate")).unwrap();
    assert_eq!(member.turn_process, last.entries[0].turn_process);
    assert_eq!(store.load_canonical().unwrap().len(), 5);
}

#[test]
fn terminal_turns_survive_reconstruction_later_attempts_and_paging() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("terminal.sqlite");
    let id = ConversationId::new("conv_b05f9cb7-dcec-7fa1-8fa9-2047ae76d95f");
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    let mut expected = Vec::new();
    for (attempt, terminal, outcome) in terminal_cases() {
        user(&store, &format!("user-{attempt}"));
        append(
            &store,
            attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(attempt),
            },
        );
        append(&store, attempt, RuntimeEvent::TurnStarted);
        process_content(&store, attempt);
        let running = page(&store, None, 3);
        let live_owner = running.entries[0].turn_process.as_ref().unwrap();
        assert_eq!(live_owner.outcome, TurnProcessOutcome::Running);
        let mut terminal = event(&store, attempt, terminal);
        terminal.timestamp += chrono::Duration::seconds(7);
        store.append_event(terminal).unwrap();
        let latest = page(&store, None, 1);
        let entry = latest.entries.last().unwrap().clone();
        let RuntimeClientTranscriptItem::AttemptTerminal { turn } = &entry.item else {
            panic!("missing terminal Turn")
        };
        assert_eq!(turn.conversation_id, id);
        assert_eq!(turn.attempt_id, Some(AttemptId::new(attempt)));
        assert!(turn.attempt_id_source.is_none());
        assert_eq!(turn.outcome, outcome);
        assert_eq!(
            turn.failure.is_some(),
            outcome == TurnProcessOutcome::Failed
        );
        assert_eq!(
            turn.ended_at.unwrap() - turn.started_at.unwrap(),
            chrono::Duration::seconds(7)
        );
        assert_eq!(turn.control_cursor, live_owner.control_cursor);
        assert_eq!(turn.message_count, 2);
        assert_eq!(turn.tool_call_count, 1);
        assert_eq!(turn.final_message_id, None);
        assert!(turn.final_message_locator.is_none());
        assert!(!turn.final_message_id_deferred);
        assert!(turn.final_message_id_source.is_none());
        let members = page(&store, None, 4);
        assert!(
            members
                .entries
                .iter()
                .all(|member| member.turn_process.as_ref() == Some(turn))
        );
        assert!(
            members.entries[..3]
                .iter()
                .all(|member| member.cursor != entry.cursor)
        );
        assert_eq!(members.entries[0].cursor, turn.control_cursor);
        expected = page(&store, None, 64).entries;
    }
    // A later successful Attempt and a new running Attempt cannot rewrite history.
    assistant(&store, "later", "later-response");
    finish(&store, "later");
    append(
        &store,
        "running",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("running"),
        },
    );
    drop(store);
    let reopened = SqliteConversationStore::open_existing(id, &path).unwrap();
    let full = page(&reopened, None, 64);
    assert_eq!(&full.entries[..expected.len()], expected.as_slice());
    for size in [1, 2, 5, 64] {
        let mut before = None;
        let mut paged = Vec::new();
        loop {
            let current = page(&reopened, before, size);
            // Reverse each ascending page, then reverse the newest-to-oldest walk.
            paged.extend(current.entries.into_iter().rev());
            before = current.next_cursor.map(Into::into);
            if before.is_none() {
                break;
            }
        }
        paged.reverse();
        assert_eq!(paged, full.entries);
    }
    // Repeated resubscription reads are identical, with no live Attempt involved.
    assert_eq!(full, page(&reopened, None, 64));
    let wire = serde_json::to_string(&full).unwrap();
    assert_eq!(full, serde_json::from_str(&wire).unwrap());
}

/// Real committed reasoning, canonical Tool occurrence/result and intermediate text.
fn process_content(store: &dyn ConversationStore, attempt: &str) {
    use crate::message::types::{
        ContentBlockIndex, ReasoningBlock, ToolCallOccurrenceRef, ToolMessageBlock,
    };
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::tools::{ToolCall, ToolExecutionResult, ToolExecutionStatus};
    let id = MessageId::new(format!("{attempt}-process"));
    let call = ToolCall {
        id: ToolCallId::new("reused-provider-call"),
        tool_id: ToolId::new("bash"),
        name: "same-name".into(),
        arguments: serde_json::json!({}),
    };
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: id.clone(),
                content: vec![
                    AssistantContentBlock::Reasoning(ReasoningBlock {
                        text: Some("thinking".into()),
                        provider_state: None,
                    }),
                    AssistantContentBlock::ToolCall(call.clone()),
                ],
            }),
            event(
                store,
                attempt,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: id.clone(),
                },
            ),
        )
        .unwrap();
    store
        .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(format!("{attempt}-result")),
            occurrence: ToolCallOccurrenceRef::new(id, ContentBlockIndex::new(1)),
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        }))
        .unwrap();
    assistant(store, attempt, &format!("{attempt}-intermediate"));
}

#[test]
fn empty_terminal_uses_its_own_native_cursor() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    append(
        &store,
        "empty",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("empty"),
        },
    );
    append(
        &store,
        "empty",
        RuntimeEvent::AttemptTimedOut {
            attempt_id: AttemptId::new("empty"),
        },
    );
    let projected = page(&store, None, 1);
    let entry = &projected.entries[0];
    let owner = entry.turn_process.as_ref().unwrap();
    assert_eq!(owner.control_cursor, entry.cursor);
    assert_eq!(owner.message_count, 0);
    assert_eq!(owner.tool_call_count, 0);
    assert!(owner.started_at.is_some());
}

fn terminal_cases() -> [(&'static str, RuntimeEvent, TurnProcessOutcome); 4] {
    use crate::events::types::{AttemptFailure, AttemptLimit};
    use crate::runtime::types::{CancellationReason, RuntimeError};
    [
        (
            "cancelled",
            RuntimeEvent::AttemptCancelled {
                attempt_id: AttemptId::new("cancelled"),
                reason: CancellationReason::UserRequested,
            },
            TurnProcessOutcome::Cancelled,
        ),
        (
            "failed",
            RuntimeEvent::AttemptFailed {
                attempt_id: AttemptId::new("failed"),
                error: AttemptFailure::Runtime {
                    error: RuntimeError::Internal {
                        message: "fixture".into(),
                    },
                },
            },
            TurnProcessOutcome::Failed,
        ),
        (
            "timeout",
            RuntimeEvent::AttemptTimedOut {
                attempt_id: AttemptId::new("timeout"),
            },
            TurnProcessOutcome::TimedOut,
        ),
        (
            "limit",
            RuntimeEvent::AttemptLimitExceeded {
                attempt_id: AttemptId::new("limit"),
                limit: AttemptLimit::MaxTurns,
            },
            TurnProcessOutcome::LimitExceeded,
        ),
    ]
}

fn assert_original_tool_view(
    store: &dyn ConversationStore,
    bounded: &crate::runtime_client::snapshot::ForegroundToolExecution,
    original: &crate::runtime_client::snapshot::ForegroundToolExecution,
) {
    assert_eq!(
        bounded.committed_result, original.committed_result,
        "numeric source retains the independent original canonical Tool result"
    );
    assert!(bounded.committed_result.is_some());
    let original_message_id = original
        .message_id
        .as_ref()
        .expect("the exact original proposal alias");
    let messages = store
        .load_messages(std::slice::from_ref(original_message_id))
        .unwrap();
    let MessageBlock::Assistant(message) = &messages[0] else {
        panic!("original canonical proposal");
    };
    let AssistantContentBlock::ToolCall(call) = &message.content
        [usize::try_from(bounded.block_index.get()).expect("native block index fits")]
    else {
        panic!("the actual original Tool occurrence");
    };
    let crate::runtime_client::snapshot::ForegroundToolState::Assembled { arguments } =
        &bounded.state
    else {
        panic!("canonical C without a native Tool terminal must stay Assembled");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(arguments).unwrap(),
        call.arguments
    );
    assert_eq!(bounded.call_id.as_ref(), Some(&call.id));
    assert_eq!(bounded.tool_id.as_ref(), Some(&call.tool_id));
    assert_eq!(
        bounded.occurrence.locator,
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
            position: store
                .message_ledger_position(original_message_id)
                .unwrap()
                .unwrap(),
        }
    );
    assert_eq!(bounded.block_index, original.block_index);
}

#[test]
fn exact_window_decoration_keeps_attempt_tool_occurrences_and_response_identity() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    for attempt in ["a", "b"] {
        append(
            &store,
            attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(attempt),
            },
        );
        process_content(&store, attempt);
        assistant(&store, attempt, &format!("{attempt}-final"));
        finish(&store, attempt);
    }
    let original = page(&store, Some(crate::durable::TranscriptCursor::new(5)), 4);
    let selected = store
        .read_window_headers(
            &crate::durable::reading::ConversationWindowAt::Older {
                cut: None,
                before: crate::durable::TranscriptCursor::new(5),
            },
            4,
        )
        .unwrap();
    let frozen_cut = selected.cut.read.clone();
    let mut expected = crate::runtime_client::snapshot::transcript_header_view(selected.page);
    super::decorate_window(&store, &mut expected, selected.cut.read.journal).unwrap();
    assert_original_tool_view(
        &store,
        &expected.entries[0].tool_calls[0],
        &original.entries[0].tool_calls[0],
    );
    let outline = store.conversation_turns(0, 64).unwrap();
    let frontier = store.presentation_frontier().unwrap();
    let unrelated = (0..2048)
        .map(|index| {
            let id = MessageId::new(format!("unrelated-{index}"));
            let mut committed = event(
                &store,
                "unrelated",
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: id.clone(),
                },
            );
            committed.event_id = EventId::new(format!("event-{}", frontier + index + 1));
            (
                AssistantMessageBlock {
                    id: id.clone(),
                    content: vec![AssistantContentBlock::Text(TextBlock {
                        text: id.to_string(),
                    })],
                },
                committed,
            )
        })
        .collect::<Vec<_>>();
    crate::durable::sqlite::append_assistant_history_fixture(&store, &unrelated).unwrap();
    let old = store
        .read_window_headers(
            &crate::durable::reading::ConversationWindowAt::Older {
                cut: Some(frozen_cut),
                before: outline.turns[1].cursor.unwrap(),
            },
            4,
        )
        .unwrap();
    let mut retained = crate::runtime_client::snapshot::transcript_header_view(old.page);
    super::decorate_window(&store, &mut retained, old.cut.read.journal).unwrap();
    assert_eq!(
        retained, expected,
        "the exact old native cut remains immutable after unrelated commits"
    );
    // The four rows of Attempt a: the page before Attempt b's location.
    let read = store
        .read_window_headers(
            &crate::durable::reading::ConversationWindowAt::Older {
                cut: None,
                before: outline.turns[1].cursor.unwrap(),
            },
            4,
        )
        .unwrap();
    let mut window = crate::runtime_client::snapshot::transcript_header_view(read.page);
    super::decorate_window(&store, &mut window, read.cut.read.journal).unwrap();
    assert_eq!(window.entries, expected.entries);
    assert_eq!(
        tails(&window)[0].origin.attempt_id,
        Some(AttemptId::new("a"))
    );
    assert!(window.entries.iter().all(|entry| {
        entry
            .turn_process
            .as_ref()
            .is_some_and(|owner| owner.attempt_id == Some(AttemptId::new("a")))
    }));
    assert_eq!(
        window.entries[0].tool_calls[0].state,
        expected.entries[0].tool_calls[0].state
    );
    assert!(window.statistics.is_none());
    assert!(store.conversation_read_cut().unwrap().journal > outline.cut.journal);
}

#[test]
fn model_failure_keeps_normalized_diagnostics_without_provider_fields() {
    use crate::events::types::AttemptFailure;
    use crate::model::{MalformedToolProposalSource, ModelError};
    for has_content in [false, true] {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        append(
            &store,
            "invalid",
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("invalid"),
            },
        );
        if has_content {
            assistant(&store, "invalid", "partial");
        }
        append(
            &store,
            "invalid",
            RuntimeEvent::AttemptFailed {
                attempt_id: AttemptId::new("invalid"),
                error: AttemptFailure::Model {
                    error: ModelError {
                        provider_code: Some("private-provider-code".into()),
                        ..ModelError::malformed_tool_proposal(
                            MalformedToolProposalSource::AdapterStructural,
                            "Invalid tool arguments",
                        )
                    },
                },
            },
        );
        let projected = page(&store, None, 1);
        let owner = projected.entries[0].turn_process.as_ref().unwrap();
        let failure = serde_json::to_value(owner.failure.as_ref().unwrap()).unwrap();
        assert_eq!(failure["kind"], "malformed_tool_proposal");
        assert_eq!(failure["message"], "Invalid tool arguments");
        assert!(failure.get("provider_code").is_none());
        assert!(projected.entries[0].completed_response.is_none());
    }
}

#[test]
fn agent_statistics_replays_incrementally_with_full_native_metrics() {
    use crate::runtime_client::agent_statistics::StatisticsFold;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    let mut fold = StatisticsFold::default();
    let base = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let append_at = |attempt: &str, seconds, kind| {
        let mut envelope = event(&store, attempt, kind);
        envelope.timestamp = base + chrono::Duration::seconds(seconds);
        store.append_event(envelope).unwrap();
    };
    append_at(
        "one",
        0,
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("one"),
        },
    );
    request(&store, "one", 0, Some(usage(Some(80))));
    assistant(&store, "one", "response-one");
    append_at(
        "one",
        10,
        RuntimeEvent::AttemptCompleted {
            attempt_id: AttemptId::new("one"),
            finish_reason: ModelFinishReason::Stop,
        },
    );
    let cut = store.presentation_frontier().unwrap();
    let first_cut = cut;
    let first = fold.read(&store, cut, || None).unwrap();
    assert_eq!(first.statistics, statistics(&store, cut).unwrap());
    let native_occupancy = crate::context::occupancy::read(&store, cut).unwrap();
    assert!(
        native_occupancy
            .as_ref()
            .is_some_and(|value| value.model.len() <= 512)
    );
    let expected_occupancy = native_occupancy.as_ref().map(|value| {
        crate::runtime_client::occupancy::RuntimeClientContextOccupancy::from_native(value, None)
    });
    assert_eq!(first.occupancy, expected_occupancy);
    assert!(
        first
            .occupancy
            .as_ref()
            .is_some_and(|value| !value.model_deferred && value.model_source.is_none())
    );
    assert!(
        first
            .statistics
            .timing
            .as_ref()
            .unwrap()
            .output_tokens_per_second
            .is_some()
    );
    assert_eq!(first.duration.settled_ms, 10_000);
    assert_eq!(first, fold.read(&store, cut, || None).unwrap());

    // A later activation accumulates work, never the ninety-second idle gap.
    append_at(
        "two",
        100,
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("two"),
        },
    );
    request(&store, "two", 0, None);
    append_at("two", 110, RuntimeEvent::TurnStarted);
    let cut = store.presentation_frontier().unwrap();
    let live = fold
        .read(&store, cut, || Some(base + chrono::Duration::seconds(100)))
        .unwrap();
    assert_eq!(live.statistics.model_requests, 2);
    assert_eq!(live.statistics.requests_with_usage, 1);
    assert_eq!(live.statistics.reported_usage.unwrap().total_tokens, 120);
    assert_eq!(live.occupancy, first.occupancy);
    assert!(live.duration.active.as_ref().unwrap().running);
    assert_eq!(live.duration.settled_ms, 10_000);
    let frozen = fold.read(&store, cut, || None).unwrap();
    assert!(!frozen.duration.active.as_ref().unwrap().running);
    // A new activation must not restart the abandoned previous clock.
    assert!(
        !fold
            .read(&store, cut, || Some(base + chrono::Duration::seconds(200)))
            .unwrap()
            .duration
            .active
            .unwrap()
            .running
    );
    drop(store);
    let store = SqliteConversationStore::open_existing(id, &path).unwrap();
    assert_eq!(first, fold.read(&store, first_cut, || None).unwrap());
    assert_eq!(frozen, fold.read(&store, cut, || None).unwrap());
    assert_eq!(
        frozen,
        StatisticsFold::default()
            .read(&store, cut, || None)
            .unwrap()
    );
}

#[test]
fn occupancy_compaction_absence_invalid_window_and_failed_usage_are_boundaries() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    request(&store, "a", 0, Some(usage(None)));
    let measured_cut = store.presentation_frontier().unwrap();
    let measured = crate::context::occupancy::read(&store, measured_cut)
        .unwrap()
        .unwrap();
    let snapshot = start_request_with_window(&store, "a", 1, 0);
    append(
        &store,
        "a",
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id,
            finish_reason: ModelFinishReason::Stop,
            usage: Some(usage(None)),
            generation: None,
        },
    );
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        None,
        "an invalid window is not a reason to select older usage"
    );
    assert_eq!(
        crate::context::occupancy::read(&store, measured_cut).unwrap(),
        Some(measured)
    );
    request_timed(&store, "a", 2, Some(usage(None)), None, true);
    let failed_cut = store.presentation_frontier().unwrap();
    assert!(
        !crate::context::occupancy::read(&store, failed_cut)
            .unwrap()
            .unwrap()
            .estimated
    );
    // The same native atomic compaction transition owns an explicit absent estimate.
    user(&store, "compacted-input");
    assistant(&store, "a", "compacted-response");
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "absent-summary",
                    2,
                    "summary",
                ),
                None,
            ),
        )
        .unwrap();
    request(&store, "a", 3, None);
    assert_eq!(
        crate::context::occupancy::read(&store, store.presentation_frontier().unwrap()).unwrap(),
        None,
        "unmeasured requests must not resurrect pre-compaction usage"
    );
    assert!(
        crate::context::occupancy::read(&store, failed_cut)
            .unwrap()
            .is_some()
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One inherited window/occupancy cut sequence across reopen and local compaction.
fn occupancy_reopens_and_inherits_without_local_requests_or_events() {
    let directory = tempfile::tempdir().unwrap();
    let id = ConversationId::generate();
    let path = directory.path().join("source.sqlite");
    let source = SqliteConversationStore::open(id.clone(), &path).unwrap();
    user(&source, "input");
    append(
        &source,
        "a",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("a"),
        },
    );
    request(&source, "a", 0, Some(usage(None)));
    assistant(&source, "a", "response");
    finish(&source, "a");
    let cut = source.presentation_frontier().unwrap();
    let expected = crate::context::occupancy::read(&source, cut).unwrap();
    drop(source);
    let source = SqliteConversationStore::open_existing(id, &path).unwrap();
    assert_eq!(
        crate::context::occupancy::read(&source, cut).unwrap(),
        expected
    );
    let selected = source
        .read_lineage_cut(source.load_head().unwrap().revision)
        .unwrap();
    let seed = crate::durable::LineageSeed::replayed(selected.canonical, selected.surface_history)
        .unwrap()
        .with_completed_responses(selected.completed_responses)
        .unwrap()
        .with_turns(selected.turns)
        .unwrap();
    let child_id = ConversationId::generate();
    let child_path = directory.path().join("child.sqlite");
    let child = SqliteConversationStore::open(child_id.clone(), &child_path).unwrap();
    child.initialize_lineage(&seed).unwrap();
    assert_eq!(
        crate::context::occupancy::read(&child, 0).unwrap(),
        expected
    );
    assert_eq!(child.presentation_frontier().unwrap(), 0);
    assert!(
        child
            .transcript_attempts(&[MessageId::new("response")], 0)
            .unwrap()
            .is_empty()
    );
    let selected_window = child
        .read_window_headers(&crate::durable::reading::ConversationWindowAt::Latest, 8)
        .unwrap();
    let mut copied_page = super::super::snapshot::transcript_header_view(selected_window.page);
    super::decorate_window(&child, &mut copied_page, selected_window.cut.read.journal).unwrap();
    assert_eq!(
        tails(&copied_page)[0].origin.conversation_id,
        source.conversation_id().clone()
    );

    drop(child);
    let child = SqliteConversationStore::open(child_id, &child_path).unwrap();
    assert_eq!(
        crate::context::occupancy::read(&child, 0).unwrap(),
        expected
    );
    for retry in 0..2048 {
        request(&child, "child", retry, None);
    }
    assert_eq!(
        crate::context::occupancy::read(&child, child.presentation_frontier().unwrap()).unwrap(),
        expected
    );
    let before_compaction = child.presentation_frontier().unwrap();
    child
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &child,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &child,
                    "child-summary",
                    2,
                    "summary",
                ),
                None,
            ),
        )
        .unwrap();
    assert_eq!(
        crate::context::occupancy::read(&child, 0).unwrap(),
        expected
    );
    assert_eq!(
        crate::context::occupancy::read(&child, before_compaction).unwrap(),
        expected
    );
    assert_eq!(
        crate::context::occupancy::read(&child, child.presentation_frontier().unwrap()).unwrap(),
        None
    );
}

/// Capacity suite: this test's value comes from its scale, so FAST skips
/// `::scale::` and FULL runs it (tests/README.md, "Capacity tests").
mod scale {
    use super::*;
    use crate::durable::JournalStore;

    #[test]
    fn occupancy_thousands_of_unmeasured_terminal_requests_do_not_invent_a_reading() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        for retry in 0..2048 {
            request_timed(&store, "unmeasured", retry, None, None, retry % 2 == 0);
        }
        assert_eq!(
            crate::context::occupancy::read(&store, store.presentation_frontier().unwrap())
                .unwrap(),
            None
        );
        let cut = store.presentation_frontier().unwrap();
        request_timed(&store, "unmeasured", 2048, Some(usage(None)), None, true);
        assert_eq!(crate::context::occupancy::read(&store, cut).unwrap(), None);
        assert_eq!(
            crate::context::occupancy::read(&store, store.presentation_frontier().unwrap())
                .unwrap()
                .unwrap()
                .input_tokens,
            100
        );
    }
}

#[test]
fn window_decoration_empty_human_only_and_before_local_ownership_remain_exact() {
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    let read = store
        .read_window_headers(&crate::durable::reading::ConversationWindowAt::Latest, 8)
        .unwrap();
    let mut empty = super::super::snapshot::transcript_header_view(read.page);
    super::decorate_window(&store, &mut empty, read.cut.read.journal).unwrap();
    assert!(empty.entries.is_empty());
    user(&store, "human-only");
    let read = store
        .read_window_headers(&crate::durable::reading::ConversationWindowAt::Latest, 8)
        .unwrap();
    let mut human = super::super::snapshot::transcript_header_view(read.page);
    super::decorate_window(&store, &mut human, read.cut.read.journal).unwrap();
    assert!(human.entries[0].turn_process.is_none());
    assert!(human.entries[0].completed_response.is_none());
    let frozen = human.clone();
    append(
        &store,
        "later",
        RuntimeEvent::AttemptStarted {
            attempt_id: AttemptId::new("later"),
        },
    );
    assistant(&store, "later", "later-answer");
    finish(&store, "later");
    let read = store
        .read_window_headers(&crate::durable::reading::ConversationWindowAt::Latest, 8)
        .unwrap();
    let mut historical = super::super::snapshot::transcript_header_view(read.page);
    let mut old_window = frozen.clone();
    super::decorate_window(&store, &mut old_window, 0).unwrap();
    assert_eq!(old_window, frozen, "the genuine old native cut stays exact");
    assert!(matches!(
        super::decorate_window(&store, &mut historical, 0),
        Err(crate::durable::ConversationStoreError::InvalidReference(_))
    ));
    assert!(tails(&old_window).is_empty());
    assert!(
        old_window
            .entries
            .iter()
            .all(|entry| entry.turn_process.is_none())
    );
    super::decorate_window(&store, &mut historical, read.cut.read.journal).unwrap();
    assert_eq!(
        tails(&historical)[0].origin.attempt_id,
        Some(AttemptId::new("later"))
    );
}

#[test]
fn cutless_response_decoration_is_rejected_without_changing_native_page() {
    let store = SqliteConversationStore::in_memory(ConversationId::new(
        "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b",
    ))
    .unwrap();
    store.initialize(&[]).unwrap();
    let mut page =
        super::super::snapshot::transcript_page_view(store.load_transcript_page(None, 1).unwrap())
            .unwrap();
    let original = page.clone();
    for window in [false, true] {
        let result = if window {
            decorate_window(&store, &mut page, 0)
        } else {
            decorate_through(&store, &mut page, 0)
        };
        assert!(matches!(
            result,
            Err(ConversationStoreError::InvalidReference(ref detail))
                if detail == "native decoration cut absent"
        ));
        assert_eq!(page, original);
    }
}
