//! Summary total-output recovery preserves its frozen reasoning invocation.

use std::sync::{Arc, Mutex};

use super::super::support;
use rustx::context::{CompactionConstraints, ContextRuntime, DefaultTokenEstimator};
use rustx::durable::{CanonicalStore, JournalStore, ProjectionStore, SqliteConversationStore};
use rustx::message::content::TextBlock;
use rustx::message::types::{
    ContentBlockIndex, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use rustx::model::catalog::{MapCredentialEnvironment, ModelCatalog, ModelRef};
use rustx::model::session::{SessionModelConfig, SessionModelState, SummaryModelPolicy};
use rustx::model::structured::StructuredOutputStrategy;
use rustx::model::{
    ModelAdapter, ModelEvent, ModelFinishReason, ModelProtocol, ModelRequest, ModelStream,
    ModelStreamItem, RequestParams,
};
use rustx::runtime::CancellationSignal;
use rustx::runtime::identity::{ConversationId, MessageId};
use support::model::{FixtureModel, ScriptedAdapterFactory, fixture_catalog_document};

#[derive(Default)]
struct ReasoningModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl ModelAdapter for ReasoningModel {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, request: ModelRequest, _: CancellationSignal) -> ModelStream {
        let exhausted = request.max_output_tokens() < 7_000;
        self.requests.lock().unwrap().push(request);
        let mut events = vec![ModelEvent::Started];
        if !exhausted {
            events.push(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: serde_json::json!({
                    "goal": "Continue the task",
                    "constraints_and_preferences": [],
                    "progress": {"done": [], "in_progress": [], "blocked": []},
                    "key_decisions": [], "next_steps": [], "critical_context": []
                })
                .to_string(),
            });
        }
        events.push(ModelEvent::Completed {
            finish_reason: if exhausted {
                ModelFinishReason::Length
            } else {
                ModelFinishReason::Stop
            },
            usage: None,
        });
        Box::pin(futures_util::stream::iter(
            events.into_iter().map(ModelStreamItem::Event),
        ))
    }
}

fn user(id: &str, text: String) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock { text })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One operation proves recovery, exact coverage and atomic activation.
async fn reasoning_output_growth_reprices_fragments_and_commits_once() {
    let adapter = Arc::new(ReasoningModel::default());
    let mut document = fixture_catalog_document(&[
        FixtureModel::text("primary/small", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(16_000)
            .with_max_output_tokens(1_000),
        FixtureModel::text("summary/reasoning", ModelProtocol::OpenAiChatCompletions)
            .with_context_window(128_000)
            .with_max_output_tokens(64_000)
            .with_profiles(
                "thinking",
                serde_json::json!({
                    "thinking": {"requestParams": {"reasoning_effort": "high"}}
                }),
            ),
    ]);
    document
        .models
        .get_mut("summary/reasoning")
        .unwrap()
        .capabilities
        .json_schema_output = true;
    let catalog = ModelCatalog::from_document(document)
        .unwrap()
        .resolve(&MapCredentialEnvironment::default())
        .unwrap();
    let registry = rustx::model::invocation::ModelBindingRegistry::new_with_scripted_adapters(
        catalog,
        &ScriptedAdapterFactory::new(adapter.clone()),
    )
    .unwrap();
    let mut config = SessionModelConfig::of(ModelRef::parse("primary/small").unwrap());
    config.summary_model = SummaryModelPolicy::Explicit {
        model: ModelRef::parse("summary/reasoning").unwrap(),
        profile: None,
        request_params: RequestParams::new(),
    };
    let snapshot = SessionModelState::new(registry, config)
        .unwrap()
        .snapshot()
        .unwrap();
    let clock = support::default_monotonic_clock();
    let context = ContextRuntime::for_attempt(
        Arc::new(DefaultTokenEstimator),
        None,
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        clock.clone(),
    )
    .unwrap();
    let conversation_id = ConversationId::generate();
    let store = Arc::new(SqliteConversationStore::in_memory(conversation_id.clone()).unwrap());
    let history = vec![
        user("old", "historical ".repeat(50_000)),
        user("fresh", "Continue".into()),
    ];
    store.initialize(&history).unwrap();
    context.summarizer.bind_backend(
        Arc::new(
            crate::runtime::auxiliary::executor::AuxiliaryExecutor::for_test(store.clone(), clock)
                .unwrap(),
        ),
        0,
    );
    let head = store.load_head().unwrap();
    let mut state = rustx::conversation::ConversationState::from_durable_head(
        history.clone(),
        head.active_message_ids,
        head.revision,
        head.compaction_generation,
    )
    .unwrap();
    let fresh =
        rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("fresh")]).unwrap();
    let attribution = crate::context::compaction::CompactionAttribution::default();
    crate::context::compaction::execute_compaction(
        &mut state,
        &context,
        &conversation_id,
        store.as_ref(),
        &[],
        None,
        "",
        &CompactionConstraints {
            fresh_inbound: Some(&fresh),
            ..Default::default()
        },
        &CancellationSignal::new(),
        attribution,
    )
    .await
    .expect("reasoning Length recovers without changing the selected Profile");

    let requests = adapter.requests.lock().unwrap();
    assert!(
        (3..=5).contains(&requests.len()),
        "bounded retry and multiple accepted ranges"
    );
    assert_eq!(requests[0].max_output_tokens(), 3_550);
    assert!(
        serde_json::to_string(&requests[1].messages).unwrap().len()
            < serde_json::to_string(&requests[0].messages).unwrap().len(),
        "larger output reservation must reprice and shrink the unprocessed fragment"
    );
    for request in &requests[1..] {
        assert_eq!(request.max_output_tokens(), 7_100);
    }
    for request in requests.iter() {
        let mut expected = requests[0].invocation.clone();
        expected.max_output_tokens = request.max_output_tokens();
        assert_eq!(request.invocation, expected);
        assert_eq!(request.response_schema, requests[0].response_schema);
        assert_eq!(
            request.invocation.request_params["reasoning_effort"],
            "high"
        );
        assert_eq!(
            request.response_schema.as_ref().unwrap().strategy,
            StructuredOutputStrategy::Native
        );
        assert_eq!(
            request.invocation.model,
            snapshot.summary_invocation().invocation_config().model
        );
    }
    let events = store.read_events(None, 128).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event,
                rustx::events::types::RuntimeEvent::CompactionCompleted { .. }
            ))
            .count(),
        1
    );
    assert!(!events.iter().any(|event| matches!(
        event.event,
        rustx::events::types::RuntimeEvent::CompactionFailed { .. }
            | rustx::events::types::RuntimeEvent::CompactionCancelled { .. }
            | rustx::events::types::RuntimeEvent::CompactionSkipped { .. }
    )));
    let ranges: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            rustx::events::types::RuntimeEvent::CompactionChunkAccepted {
                source_ranges, ..
            } => Some(source_ranges),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(ranges.len() > 1);
    let mut cursor = 0;
    for range in ranges {
        assert_eq!(range.projection_index, 0);
        assert_eq!(range.byte_start, cursor);
        cursor = range.byte_end;
    }
    let rendered = context
        .engine
        .rendered_input(&rustx::model::input::canonical_input(&history).into())
        .unwrap();
    let mut original = String::new();
    crate::context::summarizer::append_summary_message(&rendered[0], &mut original);
    assert_eq!(
        cursor as usize,
        original.len(),
        "all rendered source bytes covered once"
    );
    assert_eq!(
        store
            .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 64)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.load_canonical().unwrap(), history);
    let projected = store.projected_history().unwrap();
    assert_eq!(projected.messages.len(), 2);
    assert_eq!(
        projected.messages[1].canonical_id(),
        Some(&MessageId::new("fresh"))
    );
}
