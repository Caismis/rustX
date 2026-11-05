//! The compaction pipeline owner: the shared committed transition
//!
//! ```text
//! plan -> summarize -> validate exact post-summary fit -> durable commit
//!       -> hot-state installation
//! ```
//!
//! Two halves, both deterministic and network-free:
//!
//! - the **summarize stage** contract: the adapter-backed
//!   `ModelBackedSummarizer` issues a canonical one-off request (no tools,
//!   no continuation, the resolved summary invocation) and rejects invalid
//!   provider streams deterministically;
//! - the **shared transition** itself, driven through the real
//!   `execute_compaction` implementation that automatic and manual
//!   compaction both use, against a real `SqliteConversationStore`: one
//!   committed transition, and failure atomicity — cancellation before or
//!   during summary, summarizer failure, post-summary cannot-fit, and
//!   durable commit failure each install no partial durable or hot-state
//!   mutation.
//!
//! Lineage/metadata extraction contracts live in `compaction_metadata.rs`;
//! boundary invocation through `AgentExecution`/`ConversationRuntime` lives
//! in `runtime_integration.rs`.

use super::super::support;
use rustx::durable::{CanonicalStore, JournalStore, ProjectionStore};

use std::sync::Arc;

use rustx::context::{
    AgentStatusConfig, AgentStatusEngine, CompactionBudgets, CompactionConstraints, ContextConfig,
    ContextEngine, ContextErrorKind, ContextRuntime, ModelBackedSummarizer, SummaryRequest,
};
use rustx::conversation::ConversationState;
use rustx::durable::{ConversationStore, SqliteConversationStore};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::model::types::ModelUsage;
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::{ConversationId, MessageId};
use support::context::ScriptedEstimator;
use support::context::{FakeContextSummarizer, FakeSummaryStep};
use support::fake::{FakeModel, FakeStep, ScriptedCall, fake_model, tool_call_events};

use super::compaction_metadata::{file_operation_history, user as md_user};

/// A final-fit carryover downgrade must reuse all accepted source chunks.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One real-Store transition, two chunks, and final-fit fallback.
async fn carryover_degradation_reuses_accepted_chunks_before_one_commit() {
    use crate::context::compaction::{CompactionAttribution, execute_compaction_in_operation};
    use rustx::context::CompactionCarryover;
    use rustx::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use rustx::model::{
        CarryoverDetailLevel, ModelInputMessage, RenderedCarryoverRecord,
        RenderedCarryoverToolCall, RenderedUnresolvedOutputCarryover, RequestOnlyInsertionAnchor,
        RequestOnlyModelContext, UnresolvedOutputSettlement,
    };
    use rustx::runtime::identity::{EventId, PublicationStreamId, ToolCallId, ToolId};

    let history = vec![
        user_message("a", "marker-a"),
        user_message("b", "marker-b"),
        user_message("recent", "recent"),
    ];
    let store = SqliteConversationStore::in_memory(conversation()).unwrap();
    store.initialize(&history).unwrap();
    let head = store.load_head().unwrap();
    let mut state = ConversationState::from_durable_head(
        history.clone(),
        head.active_message_ids,
        head.revision,
        head.compaction_generation,
    )
    .unwrap();
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        let text: String = messages
            .iter()
            .filter_map(|message| match message.view() {
                rustx::model::projected::ModelMessageView::User(content) => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|block| match block {
                UserContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        if text.contains("<retired-conversation>") {
            return if text.contains("marker-a") && text.contains("marker-b") {
                300
            } else {
                100
            };
        }
        messages
            .iter()
            .map(|message| match message {
                ModelInputMessage::Canonical(block) if block.id().as_str() == "recent" => 100,
                ModelInputMessage::Derived(_) if text.contains("## Goal") => 10,
                ModelInputMessage::Canonical(_) | ModelInputMessage::Derived(_) => 300,
                ModelInputMessage::RequestOnly(
                    RequestOnlyModelContext::UnresolvedOutputCarryover(carryover),
                ) => {
                    if carryover.records.iter().any(|record| {
                        matches!(record,
                    RenderedCarryoverRecord::ProposedToolCall(call) if call.arguments.is_some())
                    }) {
                        250
                    } else {
                        50
                    }
                }
                ModelInputMessage::RequestOnly(_) => 0,
            })
            .sum()
    });
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![
        FakeSummaryStep::Return("S".repeat(300)),
        FakeSummaryStep::Return("S".repeat(300)),
    ]));
    let context = ContextRuntime::with_scripted_summarizer(
        ContextEngine::new(
            ContextConfig {
                context_window_tokens: 600,
                reserve_tokens: 0,
                keep_recent_tokens: 100,
            },
            Arc::new(estimator),
        )
        .unwrap(),
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 100, 200),
    );
    let attribution = CompactionAttribution::default();
    store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("carryover-start"),
            sequence: 0,
            tool_occurrence: None,
            observation: None,
            conversation_id: conversation(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            event: RuntimeEvent::CompactionStarted {
                operation_id: attribution.operation_id.clone(),
            },
        })
        .unwrap();
    let cancellation = CancellationSignal::new();
    let budget = context.compaction_budget(attribution.operation_id.clone());
    let operation = context
        .summarizer
        .operation(budget, cancellation.child())
        .unwrap();
    let carryover = RenderedUnresolvedOutputCarryover {
        source_stream_id: PublicationStreamId::new("frozen-carryover"),
        source_settlement: UnresolvedOutputSettlement::Incomplete,
        records: vec![RenderedCarryoverRecord::ProposedToolCall(
            RenderedCarryoverToolCall {
                call_id: ToolCallId::new("call"),
                tool_id: ToolId::new("tool"),
                name: "tool".into(),
                complete: false,
                arguments: Some("partial".into()),
                omitted_argument_bytes: 0,
            },
        )],
        omitted_blocks: rustx::model::CarryoverOmissionCounts::default(),
    };
    let fallbacks = [CompactionCarryover {
        level: CarryoverDetailLevel::Reduced,
        value: Some(carryover.degraded(CarryoverDetailLevel::Reduced)),
    }];
    let protected = [MessageId::new("recent")];
    let anchor = RequestOnlyInsertionAnchor::AfterCanonical;
    let constraints = CompactionConstraints {
        protected_context_ids: &protected,
        carryover: Some(&carryover),
        carryover_anchor: Some(&anchor),
        carryover_fallbacks: &fallbacks,
        ..Default::default()
    };
    let progress_count = std::sync::atomic::AtomicUsize::new(0);
    let progress = |_| {
        progress_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    };
    let completed = execute_compaction_in_operation(
        &mut state,
        &context,
        &conversation(),
        &store,
        &[],
        None,
        "",
        &constraints,
        &cancellation,
        attribution,
        operation.as_ref(),
        Some(&progress),
    )
    .await
    .unwrap();
    operation.close().await;
    assert_eq!(
        completed.carryover_level,
        Some(CarryoverDetailLevel::Reduced)
    );
    assert_eq!(
        progress_count.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "scripted summaries cannot publish native Call evidence"
    );
    let requests = summarizer.requests();
    assert_eq!(
        requests.len(),
        2,
        "each source fragment was summarized exactly once"
    );
    assert_eq!(
        requests[0].fragment.as_ref().unwrap().ranges[0].projection_index,
        0
    );
    assert_eq!(
        requests[1].fragment.as_ref().unwrap().ranges[0].projection_index,
        1
    );
    assert!(requests[1].previous.is_some());
    assert_eq!(store.load_canonical().unwrap(), history);
    assert!(store.projection_head().unwrap().is_some());
}

#[tokio::test]
async fn reduction_format_change_reprices_without_repeating_accepted_source() {
    let history = vec![user_message("a", "marker-a"), user_message("b", "marker-b")];
    let store = SqliteConversationStore::in_memory(conversation()).unwrap();
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        let text: String = messages
            .iter()
            .filter_map(|message| match message.view() {
                rustx::model::projected::ModelMessageView::User(content) => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|block| match block {
                UserContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        if text.contains("<retired-conversation>") {
            if (text.contains("marker-a") && text.contains("marker-b"))
                || (text.contains("<previous-accepted-summary>\nexpensive")
                    && text.contains("[source "))
            {
                300
            } else {
                100
            }
        } else {
            messages
                .iter()
                .map(|message| {
                    if matches!(message, rustx::model::ModelInputMessage::Derived(_)) {
                        10
                    } else {
                        300
                    }
                })
                .sum()
        }
    });
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![
        FakeSummaryStep::Return("expensive".into()),
        FakeSummaryStep::Fail(rustx::context::ContextError::new(
            ContextErrorKind::SummaryFormatChanged,
            "downgrade",
        )),
        FakeSummaryStep::Return("longer but cheaper".into()),
        FakeSummaryStep::Return("final".into()),
    ]));
    let context = ContextRuntime::with_scripted_summarizer(
        ContextEngine::new(
            ContextConfig {
                context_window_tokens: 2_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(estimator),
        )
        .unwrap(),
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 100, 200),
    );
    run_pipeline(&history, &store, &context, &CancellationSignal::new())
        .await
        .unwrap();
    let requests = summarizer.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests[0].fragment.as_ref().unwrap().ranges[0].projection_index,
        0
    );
    assert!(requests[1].fragment.is_none() && requests[2].fragment.is_none());
    assert_eq!(requests[1].previous, requests[2].previous);
    assert_eq!(
        requests[3].fragment.as_ref().unwrap().ranges[0].projection_index,
        1
    );
    assert_eq!(
        requests[3].previous.as_ref().unwrap().text,
        "longer but cheaper"
    );
}

#[tokio::test]
async fn final_fit_reprices_a_longer_but_cheaper_reduction() {
    let history = vec![user_message("old", "historical work")];
    let store = SqliteConversationStore::in_memory(conversation()).unwrap();
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        let mut derived_text = String::new();
        for message in messages {
            if let rustx::model::ModelInputMessage::Derived(derived) = message
                && let rustx::model::projected::ProjectedMessage::User { content, .. } =
                    &derived.message
            {
                for block in content {
                    if let UserContentBlock::Text(text) = block {
                        derived_text.push_str(&text.text);
                    }
                }
            }
        }
        if derived_text.contains("<retired-conversation>") {
            1
        } else if derived_text.contains("expensive") {
            500
        } else if !derived_text.is_empty() {
            10
        } else {
            1_000
        }
    });
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![
        FakeSummaryStep::Return("expensive".into()),
        FakeSummaryStep::Return("longer but cheaper".into()),
    ]));
    let context = ContextRuntime::with_scripted_summarizer(
        ContextEngine::new(
            ContextConfig {
                context_window_tokens: 200,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(estimator),
        )
        .unwrap(),
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 100, 10_000),
    );
    run_pipeline(&history, &store, &context, &CancellationSignal::new())
        .await
        .unwrap();
    assert_eq!(summarizer.requests().len(), 2);
    let checkpoint = store.projection_head().unwrap().unwrap();
    let rustx::context::checkpoint::CheckpointMessage::Derived(summary) =
        &checkpoint.candidate.items[0].message
    else {
        panic!("derived Summary")
    };
    assert!(
        serde_json::to_string(&summary.message)
            .unwrap()
            .contains("longer but cheaper")
    );
}

#[tokio::test]
async fn small_window_overflow_excludes_the_known_rejected_request() {
    let history = vec![user_message("large", &"x".repeat(1_000))];
    let store = SqliteConversationStore::in_memory(conversation()).unwrap();
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        let text: String = messages
            .iter()
            .filter_map(|message| match message.view() {
                rustx::model::projected::ModelMessageView::User(content) => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|block| match block {
                UserContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect();
        if text.contains("<retired-conversation>") {
            (text.len() as u64).div_ceil(10)
        } else if matches!(
            messages.first(),
            Some(rustx::model::ModelInputMessage::Derived(_))
        ) {
            1
        } else {
            1_000
        }
    });
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![
        FakeSummaryStep::Fail(rustx::context::ContextError::new(
            ContextErrorKind::SummaryInputTooLarge,
            "provider overflow",
        )),
        FakeSummaryStep::Return("short".into()),
        FakeSummaryStep::Return("final".into()),
    ]));
    let context = ContextRuntime::with_scripted_summarizer(
        ContextEngine::new(
            ContextConfig {
                context_window_tokens: 10_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(estimator),
        )
        .unwrap(),
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 100, 2_800),
    );
    run_pipeline(&history, &store, &context, &CancellationSignal::new())
        .await
        .unwrap();
    let requests = summarizer.requests();
    assert_eq!(requests.len(), 3);
    let rejected = &requests[0].fragment.as_ref().unwrap().ranges[0];
    let accepted = &requests[1].fragment.as_ref().unwrap().ranges[0];
    let remainder = &requests[2].fragment.as_ref().unwrap().ranges[0];
    assert_eq!(rejected.byte_start, 0);
    assert_eq!(accepted.byte_start, 0);
    assert!(accepted.byte_end < rejected.byte_end);
    assert_eq!(remainder.byte_start, accepted.byte_end);
    assert_eq!(remainder.byte_end, rejected.byte_end);
}

#[tokio::test]
async fn summary_fragment_covers_the_frozen_upload_path_without_reading_the_file() {
    use rustx::model::uploads::{ResolvedUpload, UploadProjection, UploadProjectionResolver};
    #[derive(Debug)]
    struct FixedUpload(ResolvedUpload);
    impl UploadProjectionResolver for FixedUpload {
        fn resolve(
            &self,
            _: &[rustx::model::ModelInputMessage],
        ) -> Result<UploadProjection, String> {
            Ok(UploadProjection {
                files: vec![self.0.clone()],
            })
        }
    }
    let path = std::env::current_dir()
        .unwrap()
        .join(format!("unread-upload-{}.csv", uuid::Uuid::now_v7()));
    assert!(!path.exists());
    let reference = rustx::message::content::UploadedFileRef {
        batch_id: "batch".into(),
        name: "data.csv".into(),
    };
    let mut message = user_message("upload", "analyze this file");
    let MessageBlock::User(user) = &mut message else {
        unreachable!()
    };
    user.content
        .push(UserContentBlock::UploadedFile(reference.clone()));
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "short".into(),
    )]));
    let mut context = runtime(10_000, summarizer.clone());
    context
        .engine
        .set_upload_resolver(Arc::new(FixedUpload(ResolvedUpload {
            file: reference,
            path: path.to_string_lossy().into_owned(),
        })));
    let store = SqliteConversationStore::in_memory(conversation()).unwrap();
    run_pipeline(&[message], &store, &context, &CancellationSignal::new())
        .await
        .unwrap();
    let requests = summarizer.requests();
    let fragment = requests[0].fragment.as_ref().unwrap();
    assert!(fragment.text.contains(&path.to_string_lossy().to_string()));
    assert_eq!(fragment.ranges[0].projection_index, 0);
    assert!(!path.exists());
}

/// The engine used by the pipeline regressions: a large window so planning
/// admits the full active prefix unless a test sizes it down deliberately.
fn engine(window: u64) -> ContextEngine {
    ContextEngine::new(
        ContextConfig {
            context_window_tokens: window,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(10, 10, 0)),
    )
    .expect("valid context configuration")
}

fn scripted_call() -> ScriptedCall {
    ScriptedCall {
        id: "call-1",
        tool_id: "tool-alpha",
        name: "alpha",
        arguments: serde_json::json!({}),
    }
}

/// The resolved summary invocation of a scripted model, with an explicit
/// output budget.
///
/// It is built through the same catalog resolution as a primary model, so a
/// summarizer test exercises the real binding path.
fn summary_invocation(
    model: &Arc<FakeModel>,
    max_output_tokens: u32,
) -> rustx::model::ResolvedModelInvocation {
    support::attempt_model_with_window(model.clone(), "fake-model", 10_000_000, max_output_tokens)
        .summary_invocation()
        .clone()
}

fn user_message(id: &str, text: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

fn summary_request(text: &str) -> SummaryRequest {
    let source: rustx::model::ModelInputMessage = user_message("u1", text).into();
    let mut rendered = String::new();
    crate::context::summarizer::append_summary_message(&source, &mut rendered);
    SummaryRequest {
        fragment: Some(rustx::context::chunks::SummaryFragment {
            ranges: vec![rustx::model::auxiliary::AuxiliarySourceRange {
                projection_index: 0,
                role: rustx::model::auxiliary::AuxiliarySourceRole::User,
                byte_start: 0,
                byte_end: u32::try_from(rendered.len()).unwrap(),
            }],
            text: rendered,
        }),
        ..Default::default()
    }
}

fn started() -> ModelEvent {
    ModelEvent::Started
}

fn text_delta(index: u32, delta: &str) -> ModelEvent {
    ModelEvent::TextDelta {
        block_index: ContentBlockIndex::new(index),
        text: delta.to_owned(),
    }
}

fn done(reason: ModelFinishReason) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: None,
    }
}

fn done_with_usage(reason: ModelFinishReason, input_tokens: u64) -> ModelEvent {
    ModelEvent::Completed {
        finish_reason: reason,
        usage: Some(ModelUsage {
            input_tokens: Some(input_tokens),
            output_tokens: Some(4),
            total_tokens: Some(input_tokens + 4),
            details: None,
        }),
    }
}

fn fail(kind: rustx::model::ModelErrorKind, message: &str) -> ModelEvent {
    ModelEvent::Failed {
        error: rustx::model::ModelError {
            kind,
            message: message.to_owned(),
            retry_disposition: rustx::model::error::ModelRetryDisposition::Never,
            retry_after_ms: None,
            provider_code: None,
            context_overflow: None,
            malformed_tool_proposal: None,
            timeout_phase: None,
            generation: None,
        },
    }
}

fn conversation() -> ConversationId {
    ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18")
}

// ---------------------------------------------------------------------------
// The adapter-backed summarizer (the summarize stage)
// ---------------------------------------------------------------------------

/// The model-backed summarizer issues a canonical one-off request with no
/// tools, no continuation, the resolved summary invocation's
/// model/protocol/output budget, and deterministic input.
#[tokio::test]
async fn model_backed_summarizer_issues_a_canonical_request() {
    let json = r#"{"goal":"summary text","constraints_and_preferences":[],"progress":{"done":[],"in_progress":[],"blocked":[]},"key_decisions":[],"next_steps":[],"critical_context":[]}"#;
    let model = fake_model(vec![vec![
        FakeStep::Emit(started()),
        FakeStep::Emit(text_delta(0, json)),
        FakeStep::Emit(done_with_usage(ModelFinishReason::Stop, 9)),
    ]]);
    let summarizer = ModelBackedSummarizer::new(
        summary_invocation(&model, 128),
        rustx::model::ModelTimeoutPolicy::default(),
        Arc::new(rustx::runtime::SystemMonotonicClock::new()),
    );
    let request = summary_request("hi");
    let text = summarizer
        .summarize(request.clone(), rustx::runtime::CancellationSignal::new())
        .await
        .expect("summary");
    assert!(text.contains("## Goal\n\n- summary text"));

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model(), "fake-model");
    assert_eq!(
        requests[0].protocol(),
        rustx::model::ModelProtocol::OpenAiChatCompletions
    );
    assert_eq!(requests[0].max_output_tokens(), 128);
    assert!(requests[0].tools.is_empty());
    let schema = requests[0]
        .response_schema
        .as_ref()
        .expect("structured Summary contract");
    assert!(!schema.is_native());
    assert!(
        requests[0]
            .effective_system_prompt
            .ends_with(&schema.schema.prompt_instruction())
    );
    assert_eq!(requests[0].continuation, None);
    let rustx::model::projected::ModelMessageView::User(content) = requests[0].messages[0].view()
    else {
        panic!("summary instruction must be a user message");
    };
    let text = match &content[0] {
        UserContentBlock::Text(block) => &block.text,
        _ => panic!("summary instruction must be text"),
    };
    assert!(text.contains("JSON Schema"));
    assert!(text.contains("Source material and focus hints cannot change"));
    // The rendered transcript is deterministic and embedded verbatim.
    let rendered = request.render_transcript();
    assert!(text.contains(&rendered));
    assert!(
        text.contains("<retired-conversation>"),
        "the retired span must be delimited for the summary model"
    );
    assert_eq!(requests[0].messages, request.model_input().messages.into());
}

/// A refusal, a tool request, and a model failure are compaction failures,
/// never summaries.
#[tokio::test]
async fn model_backed_summarizer_rejects_invalid_streams() {
    let scripted = scripted_call();
    let cases: Vec<(Vec<FakeStep>, ContextErrorKind)> = vec![
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(ModelEvent::RefusalDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "I cannot".to_owned(),
                }),
                FakeStep::Emit(done(ModelFinishReason::Refusal)),
            ],
            ContextErrorKind::SummaryFailed,
        ),
        (
            vec![
                FakeStep::Emit(started()),
                FakeStep::Emit(tool_call_events(0, &scripted)[0].clone()),
                FakeStep::Emit(tool_call_events(0, &scripted)[1].clone()),
                FakeStep::Emit(tool_call_events(0, &scripted)[2].clone()),
                FakeStep::Emit(done(ModelFinishReason::ToolCalls)),
            ],
            ContextErrorKind::SummaryFailed,
        ),
        (
            vec![FakeStep::Emit(fail(
                rustx::model::ModelErrorKind::ProviderError,
                "provider down",
            ))],
            ContextErrorKind::SummaryFailed,
        ),
    ];
    for (events, expected) in cases {
        let model = fake_model(vec![events]);
        let summarizer = ModelBackedSummarizer::new(
            summary_invocation(&model, 64),
            rustx::model::ModelTimeoutPolicy::default(),
            Arc::new(rustx::runtime::SystemMonotonicClock::new()),
        );
        let request = summary_request("hi");
        let error = summarizer
            .summarize(request, rustx::runtime::CancellationSignal::new())
            .await
            .expect_err("must fail");
        assert_eq!(error.kind, expected);
    }
}

/// A refusal with no `RefusalDelta`, an empty `Stop` output, and a
/// whitespace-only output are compaction failures: the terminal finish
/// reason is authoritative and empty output can never be a summary.
#[tokio::test]
async fn model_backed_summarizer_rejects_refusal_without_delta_and_empty_output() {
    let cases: Vec<Vec<FakeStep>> = vec![
        // Refusal with no RefusalDelta: the finish reason alone must fail.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Refusal)),
        ],
        // Completed(Stop) with no TextDelta at all.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Whitespace-only output.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "   \n\t ")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
    ];
    for events in cases {
        let model = fake_model(vec![events]);
        let summarizer = ModelBackedSummarizer::new(
            summary_invocation(&model, 64),
            rustx::model::ModelTimeoutPolicy::default(),
            Arc::new(rustx::runtime::SystemMonotonicClock::new()),
        );
        let request = summary_request("hi");
        let error = summarizer
            .summarize(request, rustx::runtime::CancellationSignal::new())
            .await
            .expect_err("invalid summary must fail");
        assert_eq!(error.kind, ContextErrorKind::SummaryFailed);
    }
}

#[tokio::test]
async fn model_backed_summarizer_rejects_invalid_json_and_truncated_json() {
    let valid = r#"{"goal":"goal","constraints_and_preferences":[],"progress":{"done":[],"in_progress":[],"blocked":[]},"key_decisions":[],"next_steps":[],"critical_context":[]}"#;
    for (text, finish, expected) in [
        (
            "not JSON",
            ModelFinishReason::Stop,
            ContextErrorKind::SummaryFailed,
        ),
        (
            valid,
            ModelFinishReason::Length,
            ContextErrorKind::SummaryOutputExhausted,
        ),
    ] {
        let model = fake_model(vec![vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, text)),
            FakeStep::Emit(done(finish)),
        ]]);
        let summarizer = ModelBackedSummarizer::new(
            summary_invocation(&model, 1_024),
            rustx::model::ModelTimeoutPolicy::default(),
            Arc::new(rustx::runtime::SystemMonotonicClock::new()),
        );
        let error = summarizer
            .summarize(summary_request("hi"), CancellationSignal::new())
            .await
            .expect_err("incomplete or invalid output cannot be accepted");
        assert_eq!(error.kind, expected);
        assert_eq!(model.requests().len(), 1);
    }
}

/// Malformed canonical stream orderings are compaction failures: content
/// before `Started`, a duplicate `Started`, `Completed` without `Started`,
/// and events after the terminal are never folded into a summary.
#[tokio::test]
async fn model_backed_summarizer_rejects_malformed_stream_orderings() {
    let cases: Vec<Vec<FakeStep>> = vec![
        // Content before Started.
        vec![
            FakeStep::Emit(text_delta(0, "early")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Duplicate Started.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(started()),
            FakeStep::Emit(text_delta(0, "x")),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
        ],
        // Completed without Started.
        vec![FakeStep::Emit(done(ModelFinishReason::Stop))],
        // Events after the terminal event.
        vec![
            FakeStep::Emit(started()),
            FakeStep::Emit(done(ModelFinishReason::Stop)),
            FakeStep::Emit(text_delta(0, "late")),
        ],
    ];
    for events in cases {
        let model = fake_model(vec![events]);
        let summarizer = ModelBackedSummarizer::new(
            summary_invocation(&model, 64),
            rustx::model::ModelTimeoutPolicy::default(),
            Arc::new(rustx::runtime::SystemMonotonicClock::new()),
        );
        let request = summary_request("hi");
        let error = summarizer
            .summarize(request, rustx::runtime::CancellationSignal::new())
            .await
            .expect_err("malformed stream must fail");
        assert_eq!(error.kind, ContextErrorKind::SummaryFailed);
    }
}

/// Cancellation aborts the summary.
#[tokio::test]
async fn model_backed_summarizer_aborts_on_cancellation() {
    let model = fake_model(vec![vec![FakeStep::ParkUntilCancelled]]);
    let summarizer = ModelBackedSummarizer::new(
        summary_invocation(&model, 64),
        rustx::model::ModelTimeoutPolicy::default(),
        Arc::new(rustx::runtime::SystemMonotonicClock::new()),
    );
    let cancellation = rustx::runtime::CancellationSignal::new();
    let request = summary_request("hi");
    let mut parked = model.parked();
    let controller_cancellation = cancellation.clone();
    let controller = tokio::spawn(async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            parked.wait_for(|value| *value),
        )
        .await
        .expect("model park must not stall")
        .expect("model parked");
        controller_cancellation.cancel();
    });
    let error = summarizer
        .summarize(request, cancellation)
        .await
        .expect_err("cancelled");
    controller.await.expect("controller task");
    assert_eq!(error.kind, ContextErrorKind::Cancelled);
}

// ---------------------------------------------------------------------------
// The shared compaction pipeline, against a real durable store
// ---------------------------------------------------------------------------

/// A fixed deterministic status clock.
struct FixedClock;

impl rustx::context::AgentStatusClock for FixedClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(1_754_000_000, 0).expect("fixed clock")
    }
}

fn runtime(window: u64, summarizer: Arc<FakeContextSummarizer>) -> ContextRuntime {
    ContextRuntime::with_scripted_summarizer(
        engine(window),
        summarizer,
        Some(AgentStatusEngine::new(
            AgentStatusConfig::default(),
            Arc::new(FixedClock),
        )),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

/// One pipeline execution of the shared compaction transition.
async fn run_pipeline(
    history: &[MessageBlock],
    store: &dyn ConversationStore,
    context: &ContextRuntime,
    cancellation: &CancellationSignal,
) -> Result<
    crate::context::compaction::ExecutedCompaction,
    crate::context::compaction::CompactionExecutionError,
> {
    store.initialize(history).expect("initialize store");
    let head = store.load_head().expect("durable head");
    let mut state = ConversationState::from_durable_head(
        history.to_vec(),
        head.active_message_ids.clone(),
        head.revision,
        head.compaction_generation,
    )
    .expect("hot state from the durable head");
    let result = crate::context::compaction::execute_compaction(
        &mut state,
        context,
        &conversation(),
        store,
        &[],
        None,
        "",
        &CompactionConstraints::default(),
        cancellation,
        crate::context::compaction::CompactionAttribution::default(),
    )
    .await;
    if result.is_ok() {
        assert_eq!(
            store.load_canonical().unwrap(),
            history,
            "compaction preserves canonical history without a summary insertion"
        );
        let compiled = store.projected_history().unwrap();
        assert!(compiled.messages.iter().any(|m|matches!(m, rustx::model::ModelInputMessage::Derived(d) if d.transformations.contains(&rustx::model::projected::SemanticTransformation::Summary))));
    }
    result
}

/// Asserts the complete failure-atomicity contract: neither the hot state
/// nor the durable authority shows any part of the aborted transition.
fn assert_no_transition(store: &dyn ConversationStore, history: &[MessageBlock]) {
    let head = store.load_head().expect("durable head");
    assert_eq!(
        head.revision,
        rustx::conversation::SurfaceRevision::new(history.len() as u64),
        "the durable Surface revision is untouched"
    );
    assert_eq!(head.compaction_generation, 0);
    assert_eq!(store.load_canonical().unwrap(), history);
    assert!(store.projection_head().unwrap().is_none());
    assert!(
        store
            .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 64)
            .unwrap()
            .is_empty()
    );
}

/// A committed compaction installs the structured text and typed metadata in
/// one transition, and a reopen recovers them exactly from canonical durable
/// state.
#[tokio::test]
async fn committed_compaction_round_trips_through_the_durable_store() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("rustx.sqlite");
    let history = file_operation_history();
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "## Goal\n\nShip the feature.".to_owned(),
    )]));
    let context = runtime(10_000, summarizer);
    let receipt;
    {
        let store = SqliteConversationStore::open(conversation(), &path).expect("open store");
        let executed = run_pipeline(&history, &store, &context, &CancellationSignal::new())
            .await
            .expect("compaction commits");
        assert!(matches!(
            executed.persisted_event.event,
            rustx::events::RuntimeEvent::CompactionCompleted { .. }
        ));
        receipt = store.projection_head().unwrap().unwrap();
        assert_eq!(store.load_canonical().unwrap(), history);
    }
    let reopened = SqliteConversationStore::open(conversation(), &path).expect("reopen store");
    assert_eq!(reopened.projection_head().unwrap().unwrap(), receipt);
    assert_eq!(reopened.load_canonical().unwrap(), history);
    let metadata = receipt
        .candidate
        .summary_metadata
        .as_ref()
        .expect("trusted metadata");
    assert_eq!(metadata.read_files(), &["/src/a.rs".to_owned()]);
    assert_eq!(
        metadata.modified_files(),
        &["/src/b.rs".to_owned(), "/src/c.rs".to_owned()]
    );
    let history = reopened.projected_history().unwrap();
    assert!(history.messages[0].canonical_id().is_none());
    assert_eq!(
        history.cut.activation,
        crate::context::checkpoint::ProjectionActivationId(1)
    );
}

/// A summary-model failure installs no part of the transition.
#[tokio::test]
async fn summarizer_failure_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Fail(
        rustx::context::ContextError::new(
            rustx::context::ContextErrorKind::SummaryFailed,
            "scripted summary failure",
        ),
    )]));
    let context = runtime(10_000, summarizer);
    let result = run_pipeline(&history, &store, &context, &CancellationSignal::new()).await;
    assert!(result.is_err());
    assert_no_transition(&store, &history);
}

/// Cancellation observable before the compaction begins installs nothing.
#[tokio::test]
async fn cancellation_before_compaction_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "never used".to_owned(),
    )]));
    let context = runtime(10_000, summarizer);
    let cancellation = CancellationSignal::new();
    cancellation.cancel();
    let result = run_pipeline(&history, &store, &context, &cancellation).await;
    assert!(result.is_err());
    assert_no_transition(&store, &history);
}

/// Cancellation observed while the summary is parked installs nothing.
#[tokio::test]
async fn cancellation_during_summary_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = support::context::shared_summarizer(FakeContextSummarizer::new(vec![
        FakeSummaryStep::ParkUntilCancelled,
    ]));
    let context = runtime(10_000, summarizer.clone());
    let cancellation = CancellationSignal::new();
    let mut parked = summarizer.parked();
    let canceller = cancellation.clone();
    let cancel_when_parked = async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            parked.wait_for(|is_parked| *is_parked),
        )
        .await
        .expect("summary park must not stall")
        .expect("the summarizer parks");
        canceller.cancel();
    };
    let (result, ()) = tokio::join!(
        run_pipeline(&history, &store, &context, &cancellation),
        cancel_when_parked
    );
    assert!(result.is_err());
    assert_no_transition(&store, &history);
}

/// A post-summary cannot-fit failure installs nothing.
#[tokio::test]
async fn cannot_fit_after_summary_installs_nothing() {
    // The minimal valid Summary fits, but the actual generated Summary does
    // not; a reduction that makes no progress cannot activate a checkpoint.
    let history = vec![md_user("u1"), md_user("u2")];
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![
        FakeSummaryStep::Return("S".repeat(400)),
        FakeSummaryStep::Return("S".repeat(400)),
    ]));
    let context = ContextRuntime::with_scripted_summarizer(
        ContextEngine::new(
            ContextConfig {
                context_window_tokens: 80,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(ScriptedEstimator::new(100, 100, 0)),
        )
        .unwrap(),
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let result = run_pipeline(&history, &store, &context, &CancellationSignal::new()).await;
    assert!(result.is_err());
    assert_eq!(summarizer.requests().len(), 2);
    assert_no_transition(&store, &history);
}

/// Core-owned overlay cost cannot turn an expanding historical projection into progress.
#[tokio::test]
async fn staged_context_cannot_mask_historical_summary_growth() {
    for with_staged in [false, true] {
        let history = vec![user_message("u1", "old"), user_message("u2", "old")];
        let store = SqliteConversationStore::in_memory(conversation()).unwrap();
        store.initialize(&history).unwrap();
        let mut state = ConversationState::from_messages(history.clone()).unwrap();
        let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
            messages
                .iter()
                .map(|message| {
                    let text = serde_json::to_string(&message.view()).unwrap();
                    if text.contains("STAGED") {
                        1000
                    } else if text.contains("GROWING") {
                        100
                    } else {
                        10
                    }
                })
                .sum()
        });
        let context = ContextRuntime::with_scripted_summarizer(
            ContextEngine::new(
                ContextConfig {
                    context_window_tokens: 10_000,
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                },
                Arc::new(estimator),
            )
            .unwrap(),
            Arc::new(FakeContextSummarizer::new(vec![
                FakeSummaryStep::Return("GROWING".into()),
                FakeSummaryStep::Return("GROWING".into()),
            ])),
            None,
            CompactionBudgets::new(1, 1, 1_000_000),
        );
        let mut staged = user_message("ctx", "STAGED");
        let MessageBlock::User(user) = &mut staged else {
            unreachable!()
        };
        user.source = UserSource::Runtime;
        user.kind = InboundKind::Context(rustx::message::types::ContextKind::NativeEnvironment);
        let constraints = CompactionConstraints {
            staged_request_context: if with_staged {
                std::slice::from_ref(&staged)
            } else {
                &[]
            },
            ..CompactionConstraints::default()
        };
        let result = crate::context::compaction::execute_compaction(
            &mut state,
            &context,
            &conversation(),
            &store,
            &[],
            None,
            "",
            &constraints,
            &CancellationSignal::new(),
            crate::context::compaction::CompactionAttribution::default(),
        )
        .await;
        let Err(crate::context::compaction::CompactionExecutionError::Context(error)) = result
        else {
            panic!("staged Context must never certify historical growth as progress");
        };
        assert_eq!(error.kind, ContextErrorKind::NoProgress);
        assert!(error.message.contains("unchanged text"));
        assert!(error.message.contains("no historical progress"));
        assert_no_transition(&store, &history);
        assert!(store.projection_head().unwrap().is_none());
    }
}

/// A durable commit failure installs nothing, in either authority.
#[tokio::test]
async fn durable_commit_failure_installs_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).expect("store");
    store.arm_compaction_fault_script([
        crate::durable::sqlite::CompactionFaultOperation::BeforeContentInsert,
    ]);
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "## Goal\n\nShip the feature.".to_owned(),
    )]));
    let context = runtime(10_000, summarizer);
    let result = run_pipeline(&history, &store, &context, &CancellationSignal::new()).await;
    assert!(matches!(
        result,
        Err(crate::context::compaction::CompactionExecutionError::Durable(_))
    ));
    assert_no_transition(&store, &history);
}

/// SQLite commit is the linearization point. O1's receipt is parked only after
/// the Journal observer is staged and the connection released; O2 commits before
/// O1 can install hot state. The pipeline must install O2, never its own old P1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_older_receipt_installs_current_projection_head() {
    use rustx::runtime::identity::{CheckpointId, CompactionOperationId};
    let store = Arc::new(SqliteConversationStore::in_memory(conversation()).unwrap());
    let history = vec![user_message("history", "immutable fact")];
    store.initialize(&history).unwrap();
    let context = runtime(
        10_000,
        Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
            "first summary".into(),
        )])),
    );
    let (committed_tx, committed_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    store.arm_projection_commit_return_probe(move || {
        committed_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    let worker_store = store.clone();
    let worker = tokio::spawn(async move {
        let mut state = ConversationState::from_messages(history).unwrap();
        let completed = crate::context::compaction::execute_compaction(
            &mut state,
            &context,
            &conversation(),
            worker_store.as_ref(),
            &[],
            None,
            "",
            &CompactionConstraints::default(),
            &CancellationSignal::new(),
            crate::context::compaction::CompactionAttribution::default(),
        )
        .await
        .unwrap();
        (state, completed)
    });
    committed_rx.await.unwrap();
    let first = store.projection_head().unwrap().unwrap();
    let mut candidate = first.candidate.clone();
    candidate.checkpoint_id = CheckpointId::new("newer-head");
    candidate.expected_head = Some(first.candidate.checkpoint_id.clone());
    candidate.read_cut = store.compaction_resource_cut().unwrap();
    let operation = CompactionOperationId::new("newer-operation");
    let signal = CancellationSignal::new();
    let second = store
        .commit_projection(rustx::durable::projection::ProjectionCommitInput {
            grant: signal
                .authorize_compaction_commit(operation.clone())
                .unwrap(),
            operation_id: operation,
            candidate,
            owner: store.projection_owner(None).unwrap(),
            tokens_before: rustx::runtime::types::TokenMeasurement {
                input_tokens: 100,
                source: rustx::runtime::types::TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 10,
            occupancy: first.occupancy,
            turn_id: None,
            timestamp: chrono::Utc::now(),
        })
        .unwrap();
    release_tx.send(()).unwrap();
    let (state, completed) = worker.await.unwrap();
    assert_eq!(
        state.surface().compaction_generation(),
        second.checkpoint.activation.activation.0
    );
    assert_eq!(store.projection_head().unwrap().unwrap(), second.checkpoint);
    assert_ne!(
        completed.persisted_event.event_id, second.completed.event_id,
        "O1 retains its historical completion; hot installation selects current authority"
    );
}

#[tokio::test]
async fn cancellation_observed_during_planning_starts_no_summary_and_commits_nothing() {
    let history = file_operation_history();
    let store = SqliteConversationStore::in_memory(conversation()).unwrap();
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "must not run".into(),
    )]));
    let (planning, mut entered) = tokio::sync::watch::channel(false);
    let estimator = rustx::context::ClosureTokenEstimator::new(move |messages, _, _| {
        if messages
            .iter()
            .any(|m| matches!(m, rustx::model::ModelInputMessage::Derived(_)))
        {
            planning.send_replace(true);
        }
        10
    });
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 10_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(estimator),
    )
    .unwrap();
    let context = ContextRuntime::with_scripted_summarizer(
        e,
        summarizer.clone(),
        None,
        CompactionBudgets::new(1, 1, 10_000),
    );
    let cancellation = CancellationSignal::new();
    let controller = cancellation.clone();
    let cancel = async move {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            entered.wait_for(|value| *value),
        )
        .await
        .expect("planning gate must not stall")
        .unwrap();
        // The planner yields after the first complete estimate, before the
        // next evaluation or summary admission. The owner wins here.
        controller.cancel();
    };
    let (result, ()) = tokio::join!(
        run_pipeline(&history, &store, &context, &cancellation),
        cancel
    );
    assert!(
        matches!(result, Err(crate::context::compaction::CompactionExecutionError::Context(error)) if error.kind == ContextErrorKind::Cancelled)
    );
    assert!(summarizer.requests().is_empty());
    assert_no_transition(&store, &history);
    let events = store.read_events(None, 64).unwrap().events;
    assert!(matches!(events.as_slice(), [started, cancelled]
        if matches!((&started.event, &cancelled.event),
            (rustx::events::types::RuntimeEvent::CompactionStarted { operation_id: opened },
             rustx::events::types::RuntimeEvent::CompactionCancelled { operation_id: closed }) if opened == closed)
    ));
}

#[tokio::test]
async fn n1_incomplete_search_invokes_no_summary_and_leaves_no_durable_transition() {
    let history: Vec<_> = (1..=129)
        .map(|n| user_message(&format!("M{n}"), "history"))
        .collect();
    let store = SqliteConversationStore::in_memory(conversation()).unwrap();
    let summarizer = Arc::new(FakeContextSummarizer::new(vec![FakeSummaryStep::Return(
        "must not run".into(),
    )]));
    let engine = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(rustx::context::ClosureTokenEstimator::new(|_, _, _| 1000)),
    )
    .unwrap();
    let context = ContextRuntime::with_scripted_summarizer(
        engine,
        summarizer.clone(),
        None,
        CompactionBudgets::new(0, 1, 10),
    );
    store.initialize(&history).unwrap();
    let mut state = ConversationState::from_messages(history.clone()).unwrap();
    let operation = rustx::runtime::identity::CompactionOperationId::new("n1-incomplete");
    let result = crate::context::compaction::execute_compaction(
        &mut state,
        &context,
        &conversation(),
        &store,
        &[],
        None,
        "",
        &CompactionConstraints::default(),
        &CancellationSignal::new(),
        crate::context::compaction::CompactionAttribution {
            operation_id: operation.clone(),
            ..Default::default()
        },
    )
    .await;
    assert!(
        matches!(result, Err(crate::context::compaction::CompactionExecutionError::Context(error)) if error.kind == ContextErrorKind::SearchIncomplete)
    );
    assert!(summarizer.requests().is_empty());
    assert_no_transition(&store, &history);
    let events = store.read_events(None, 64).unwrap().events;
    assert!(matches!(events.as_slice(), [started, failed]
        if matches!(&started.event, rustx::events::types::RuntimeEvent::CompactionStarted { operation_id } if operation_id == &operation)
            && matches!(&failed.event, rustx::events::types::RuntimeEvent::CompactionFailed { operation_id, .. } if operation_id == &operation)
    ));
    assert!(
        store
            .projection_receipt(&operation, "unused")
            .unwrap()
            .is_none()
    );
}
