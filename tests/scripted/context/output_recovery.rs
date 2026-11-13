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
use rustx::model::session::{AuxiliaryModelPolicy, SessionModelConfig, SessionModelState};
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
async fn reasoning_output_growth_reprices_fragments_and_commits_once() {
    reasoning_output_growth(None).await;
}

#[tokio::test]
async fn explicit_auxiliary_output_cap_prevents_reasoning_growth_and_activation() {
    reasoning_output_growth(Some(4_000)).await;
}

#[allow(clippy::too_many_lines)] // One operation proves recovery, exact coverage and atomic activation.
async fn reasoning_output_growth(output_cap: Option<u32>) {
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
    config.auxiliary = AuxiliaryModelPolicy::Explicit {
        model: ModelRef::parse("summary/reasoning").unwrap(),
        profile: crate::model::AuxiliaryProfileSelection::ModelDefault,
        request_params: RequestParams::new(),
        max_output_tokens: output_cap,
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
    let result = crate::context::compaction::execute_compaction(
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
    .await;
    if let Some(cap) = output_cap {
        let error = result.expect_err("Length cannot recover beyond the frozen Auxiliary cap");
        assert!(matches!(error,
            crate::context::compaction::CompactionExecutionError::Context(ref error)
                if error.kind == rustx::context::ContextErrorKind::SummaryOutputExhausted));
        let requests = adapter.requests.lock().unwrap();
        assert!(!requests.is_empty());
        assert_eq!(requests[0].max_output_tokens(), 3_550);
        for request in requests.iter() {
            assert!(request.max_output_tokens() <= cap);
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
        }
        assert_eq!(store.load_canonical().unwrap(), history);
        assert_eq!(
            store.load_head().unwrap().compaction_generation,
            head.compaction_generation
        );
        assert!(
            store
                .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 64)
                .unwrap()
                .is_empty()
        );
        let events = store.read_events(None, 128).unwrap().events;
        assert!(!events.iter().any(|event| matches!(
            event.event,
            rustx::events::types::RuntimeEvent::CompactionCompleted { .. }
                | rustx::events::types::RuntimeEvent::CompactionChunkAccepted { .. }
        )));
        return;
    }
    result.expect("reasoning Length recovers without changing the selected Profile");

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReductionPath {
    FinalFit,
    Rolling,
    TightReduction,
}

struct ReductionModel {
    path: ReductionPath,
    learn_before_reduction: bool,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ModelAdapter for ReductionModel {
    fn protocol(&self) -> ModelProtocol {
        ModelProtocol::OpenAiChatCompletions
    }

    fn stream(&self, request: ModelRequest, _: CancellationSignal) -> ModelStream {
        let wire = serde_json::to_string(&request.messages).unwrap();
        let reduction = !wire.contains("[source ");
        let first_fragment = !reduction && !wire.contains("<previous-accepted-summary>");
        let exhausted = (reduction || (self.learn_before_reduction && first_fragment))
            && request.max_output_tokens() < 7_000;
        self.requests.lock().unwrap().push(request);
        let mut events = vec![ModelEvent::Started];
        if !exhausted {
            let critical = if first_fragment {
                let prefix = if self.path == ReductionPath::Rolling {
                    "ROLLING-EXPANDED "
                } else {
                    "FINAL-EXPANDED "
                };
                vec![format!("{prefix}{}", "a".repeat(3_700)), "b".repeat(3_800)]
            } else {
                Vec::new()
            };
            events.push(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: serde_json::json!({
                    "goal": "Continue the task",
                    "constraints_and_preferences": [],
                    "progress": {"done": [], "in_progress": [], "blocked": []},
                    "key_decisions": [], "next_steps": [], "critical_context": critical
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

/// This deterministic estimator models a summary whose continuation cannot fit
/// beside even the smallest additional fragment. All other requests use the
/// real fallback estimator; extending a penalized request cannot lower its cost.
struct RollingCapacityEstimator;

impl rustx::context::TokenEstimator for RollingCapacityEstimator {
    fn estimate_input(
        &self,
        messages: &[rustx::model::ModelInputMessage],
        prompt: &str,
        tools: &[rustx::tools::types::ModelToolDefinition],
    ) -> u64 {
        let wire = serde_json::to_string(messages).unwrap();
        let base = rustx::context::TokenEstimator::estimate_input(
            &DefaultTokenEstimator,
            messages,
            prompt,
            tools,
        );
        if wire.contains("<previous-accepted-summary>")
            && wire.contains("ROLLING-EXPANDED")
            && wire.contains("[source ")
        {
            base.saturating_add(200_000)
        } else {
            base
        }
    }

    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        rustx::context::TokenEstimator::estimate_conversation_input(
            &DefaultTokenEstimator,
            messages,
        )
    }

    fn supports_monotone_compaction(&self, _: &[rustx::model::ModelInputMessage]) -> bool {
        true
    }
}

/// The initial reduction fits, but increasing its output reservation cannot.
struct TightReductionEstimator;

impl rustx::context::TokenEstimator for TightReductionEstimator {
    fn estimate_input(
        &self,
        messages: &[rustx::model::ModelInputMessage],
        prompt: &str,
        tools: &[rustx::tools::types::ModelToolDefinition],
    ) -> u64 {
        let wire = serde_json::to_string(messages).unwrap();
        let base = rustx::context::TokenEstimator::estimate_input(
            &DefaultTokenEstimator,
            messages,
            prompt,
            tools,
        );
        if wire.contains("<previous-accepted-summary>") {
            base.max(116_000)
        } else {
            base
        }
    }

    fn estimate_conversation_input(&self, messages: &[MessageBlock]) -> u64 {
        rustx::context::TokenEstimator::estimate_conversation_input(
            &DefaultTokenEstimator,
            messages,
        )
    }

    fn supports_monotone_compaction(&self, _: &[rustx::model::ModelInputMessage]) -> bool {
        true
    }
}

#[tokio::test]
async fn final_fit_reduction_recovers_reasoning_length() {
    for learn_before_reduction in [false, true] {
        reduction_recovers_reasoning_length(ReductionPath::FinalFit, learn_before_reduction).await;
    }
}

#[tokio::test]
async fn rolling_reduction_recovers_reasoning_length() {
    reduction_recovers_reasoning_length(ReductionPath::Rolling, false).await;
}

#[tokio::test]
async fn reduction_retry_is_not_sent_when_its_repriced_input_cannot_fit() {
    reduction_recovers_reasoning_length(ReductionPath::TightReduction, false).await;
}

#[allow(clippy::too_many_lines)] // Exercise a complete native compaction, including all evidence.
async fn reduction_recovers_reasoning_length(path: ReductionPath, learn_before_reduction: bool) {
    let adapter = Arc::new(ReductionModel {
        path,
        learn_before_reduction,
        requests: Mutex::new(Vec::new()),
    });
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
    config.auxiliary = AuxiliaryModelPolicy::Explicit {
        model: ModelRef::parse("summary/reasoning").unwrap(),
        profile: crate::model::AuxiliaryProfileSelection::ModelDefault,
        request_params: RequestParams::new(),
        max_output_tokens: None,
    };
    let snapshot = SessionModelState::new(registry, config)
        .unwrap()
        .snapshot()
        .unwrap();
    let estimator: Arc<dyn rustx::context::TokenEstimator> = match path {
        ReductionPath::FinalFit => Arc::new(DefaultTokenEstimator),
        ReductionPath::Rolling => Arc::new(RollingCapacityEstimator),
        ReductionPath::TightReduction => Arc::new(TightReductionEstimator),
    };
    let clock = support::default_monotonic_clock();
    let context = ContextRuntime::for_attempt(
        estimator.clone(),
        None,
        &snapshot,
        rustx::model::ModelTimeoutPolicy::default(),
        clock.clone(),
    )
    .unwrap();
    let conversation_id = ConversationId::generate();
    let store = Arc::new(SqliteConversationStore::in_memory(conversation_id.clone()).unwrap());
    let (old, fresh_text) = match path {
        ReductionPath::FinalFit | ReductionPath::TightReduction => (
            "h".repeat(20_000),
            format!("FRESH-PROTECTED{}", "f".repeat(50_985)),
        ),
        ReductionPath::Rolling => ("historical ".repeat(50_000), "FRESH-PROTECTED".to_owned()),
    };
    let history = vec![user("old", old), user("fresh", fresh_text)];
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
    let result = crate::context::compaction::execute_compaction(
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
        crate::context::compaction::CompactionAttribution::default(),
    )
    .await;

    if path == ReductionPath::TightReduction {
        let error = result.expect_err("repriced reduction cannot fit");
        assert!(matches!(error,
            crate::context::compaction::CompactionExecutionError::Context(ref error)
                if error.kind == rustx::context::ContextErrorKind::CannotFit));
        let requests = adapter.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "no unpriced retry is sent");
        assert_eq!(requests[1].max_output_tokens(), 3_550);
        assert!(estimator.estimate_frozen_request(&requests[1]) + 3_550 <= 128_000 - 6_400);
        assert!(estimator.estimate_frozen_request(&requests[1]) + 7_100 > 128_000 - 6_400);
        assert_eq!(
            store
                .projection_timeline(rustx::context::checkpoint::ProjectionActivationId(0), 64)
                .unwrap()
                .len(),
            0
        );
        assert_eq!(store.load_canonical().unwrap(), history);
        let projected = store.projected_history().unwrap();
        assert_eq!(projected.messages.len(), 2);
        assert_eq!(
            projected.messages[0].canonical_id(),
            Some(&MessageId::new("old"))
        );
        assert_eq!(
            projected.messages[1].canonical_id(),
            Some(&MessageId::new("fresh"))
        );
        return;
    }
    result.expect("reduction Length recovers with the frozen reasoning profile");

    let requests = adapter.requests.lock().unwrap();
    assert!(
        (3..=6).contains(&requests.len()),
        "bounded source and reduction calls: {}",
        requests.len()
    );
    let reductions: Vec<_> = requests
        .iter()
        .filter(|request| {
            !serde_json::to_string(&request.messages)
                .unwrap()
                .contains("[source ")
        })
        .collect();
    if learn_before_reduction {
        assert_eq!(requests[0].max_output_tokens(), 3_550);
        assert_eq!(requests[1].max_output_tokens(), 7_100);
        assert!(
            serde_json::to_string(&requests[0].messages)
                .unwrap()
                .contains("[source ")
        );
        assert!(
            serde_json::to_string(&requests[1].messages)
                .unwrap()
                .contains("[source ")
        );
        assert_eq!(
            reductions.len(),
            1,
            "learned headroom avoids another Length"
        );
        assert_eq!(reductions[0].max_output_tokens(), 7_100);
    } else {
        assert_eq!(
            reductions.len(),
            2,
            "one Length and one successful reduction"
        );
        assert_eq!(reductions[0].max_output_tokens(), 3_550);
        assert_eq!(reductions[1].max_output_tokens(), 7_100);
    }
    for request in requests.iter() {
        let wire = serde_json::to_string(&request.messages).unwrap();
        assert!(
            !wire.contains("FRESH-PROTECTED"),
            "protected fresh input must never enter Summary requests"
        );
        let mut expected = requests[0].invocation.clone();
        expected.max_output_tokens = request.max_output_tokens();
        assert_eq!(request.invocation, expected);
        assert_eq!(
            request.invocation.request_params["reasoning_effort"],
            "high"
        );
        assert_eq!(request.response_schema, requests[0].response_schema);
        assert_eq!(
            request.response_schema.as_ref().unwrap().strategy,
            StructuredOutputStrategy::Native
        );
        assert_eq!(
            request.invocation.model,
            snapshot.summary_invocation().invocation_config().model
        );
        assert!(
            estimator.estimate_frozen_request(request) + u64::from(request.max_output_tokens())
                <= 128_000 - 6_400,
            "each actual request must be repriced at its current output reservation"
        );
    }
    for request in reductions {
        let wire = serde_json::to_string(&request.messages).unwrap();
        assert!(
            wire.contains("<summary-reduction>"),
            "the native request must explicitly instruct reduction"
        );
        assert!(wire.contains("<previous-accepted-summary>"));
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
    if path == ReductionPath::Rolling {
        assert!(ranges.len() > 1);
    } else {
        assert_eq!(ranges.len(), 1);
    }
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
        "complete contiguous source coverage"
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
