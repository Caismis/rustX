//! Deterministic Context Engine policy and typed historical planning.
//! Durable selection/activation proofs live with the SQLite projection owner;
//! summary invocation and atomic transition proofs use `compaction_pipeline`.
use super::super::support::context::ScriptedEstimator;
use rustx::context::{
    CompactionBudgets, CompactionConstraints, ContextConfig, ContextEngine, ContextErrorKind,
    DefaultTokenEstimator, ObservedAnchor, ProviderObservedInput, TokenEstimator,
};
use rustx::conversation::SurfaceRevision;
use rustx::message::{
    TextBlock,
    types::{InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource},
};
use rustx::model::{ModelInputMessage, input::canonical_input};
use rustx::runtime::identity::MessageId;
use std::sync::Arc;
fn message(n: usize) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("M{n}")),
        content: vec![UserContentBlock::Text(TextBlock {
            text: format!("message {n}"),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}
fn input(n: usize) -> Vec<ModelInputMessage> {
    canonical_input(&(1..=n).map(message).collect::<Vec<_>>())
}
fn engine(window: u64, reserve: u64, recent: u64) -> ContextEngine {
    ContextEngine::new(
        ContextConfig {
            context_window_tokens: window,
            reserve_tokens: reserve,
            keep_recent_tokens: recent,
        },
        Arc::new(ScriptedEstimator::new(100, 100, 0)),
    )
    .unwrap()
}
fn budgets() -> CompactionBudgets {
    CompactionBudgets::new(0, 50, 100_000)
}
async fn plan(
    engine: &ContextEngine,
    input: &[ModelInputMessage],
    budgets: CompactionBudgets,
    constraints: &CompactionConstraints<'_>,
) -> Result<usize, rustx::context::ContextError> {
    crate::context::planning::summary_prefix(
        engine,
        input,
        "",
        &[],
        budgets,
        constraints,
        &rustx::runtime::CancellationSignal::new(),
    )
    .await
}
#[test]
fn short_history_requires_no_compaction() {
    let e = engine(1000, 0, 0);
    let p = e.build_projection_input(SurfaceRevision::new(1), input(1), &[], None, "");
    assert!(!e.should_compact(&p, 0).unwrap());
}
#[test]
fn threshold_equality_compacts() {
    let e = engine(200, 0, 0);
    let p = e.build_projection_input(SurfaceRevision::new(2), input(2), &[], None, "");
    assert!(e.should_compact(&p, 0).unwrap());
    assert!(!e.fits_under_soft_limit(&p, 0).unwrap());
}
#[test]
fn soft_limit_respects_output_budget_and_reserve() {
    let e = engine(1000, 100, 0);
    assert_eq!(e.soft_input_limit(200).unwrap(), 700);
    assert!(e.soft_input_limit(900).is_err());
}
#[test]
fn same_context_produces_same_estimate_and_order() {
    let e = engine(1000, 0, 0);
    let a = e.build_projection_input(SurfaceRevision::new(3), input(3), &[], None, "fixed");
    let b = e.build_projection_input(SurfaceRevision::new(3), input(3), &[], None, "fixed");
    assert_eq!(a, b);
    assert_eq!(
        a.messages
            .iter()
            .filter_map(|m| m.canonical_id())
            .cloned()
            .collect::<Vec<_>>(),
        [
            MessageId::new("M1"),
            MessageId::new("M2"),
            MessageId::new("M3")
        ]
    );
}
#[tokio::test]
async fn largest_prefix_preserves_recent_content_target() {
    let e = engine(1000, 0, 200);
    assert_eq!(
        plan(&e, &input(6), budgets(), &CompactionConstraints::default())
            .await
            .unwrap(),
        4
    );
}
#[tokio::test]
async fn hard_fit_retains_the_most_content_when_recent_target_is_impossible() {
    let e = engine(250, 0, 300);
    assert_eq!(
        plan(&e, &input(6), budgets(), &CompactionConstraints::default())
            .await
            .unwrap(),
        4
    );
}
#[tokio::test]
async fn summary_reservation_changes_the_hard_fit_independently_of_primary_output() {
    let e = engine(500, 0, 0);
    let low = CompactionBudgets::new(100, 50, 100_000);
    let high = CompactionBudgets::new(100, 300, 100_000);
    assert!(
        plan(&e, &input(6), low, &CompactionConstraints::default())
            .await
            .is_ok()
    );
    assert!(
        plan(&e, &input(6), high, &CompactionConstraints::default())
            .await
            .is_ok()
    );
    let impossible = CompactionBudgets::new(100, 500, 100_000);
    assert_eq!(
        plan(&e, &input(6), impossible, &CompactionConstraints::default())
            .await
            .unwrap_err()
            .kind,
        ContextErrorKind::CannotFit
    );
}
#[tokio::test]
async fn protected_current_generation_context_cannot_be_retired() {
    let e = engine(1000, 0, 0);
    let protected = [MessageId::new("M3")];
    let constraints = CompactionConstraints {
        protected_context_ids: &protected,
        ..Default::default()
    };
    assert_eq!(
        plan(&e, &input(6), budgets(), &constraints).await.unwrap(),
        2
    );
}
#[tokio::test]
async fn protected_first_message_produces_explicit_no_progress() {
    let e = engine(1000, 0, 0);
    let protected = [MessageId::new("M1")];
    let constraints = CompactionConstraints {
        protected_context_ids: &protected,
        ..Default::default()
    };
    assert_eq!(
        plan(&e, &input(3), budgets(), &constraints)
            .await
            .unwrap_err()
            .kind,
        ContextErrorKind::NoProgress
    );
}
#[tokio::test]
async fn missing_continuation_anchor_never_silently_loosens_selection() {
    let e = engine(1000, 0, 0);
    let missing = MessageId::new("absent");
    let constraints = CompactionConstraints {
        must_cover_through: Some(&missing),
        ..Default::default()
    };
    assert_eq!(
        plan(&e, &input(3), budgets(), &constraints)
            .await
            .unwrap_err()
            .kind,
        ContextErrorKind::NoProgress
    );
}
#[tokio::test]
async fn required_continuation_anchor_must_be_covered() {
    let e = engine(1000, 0, 400);
    let anchor = MessageId::new("M3");
    let constraints = CompactionConstraints {
        must_cover_through: Some(&anchor),
        ..Default::default()
    };
    assert!(plan(&e, &input(6), budgets(), &constraints).await.unwrap() >= 3);
}
#[tokio::test]
async fn staged_core_context_is_charged_as_a_whole_request() {
    let e = engine(450, 0, 0);
    let staged = vec![message(7), message(8), message(9), message(10), message(11)];
    let constraints = CompactionConstraints {
        staged_request_context: &staged,
        ..Default::default()
    };
    assert_eq!(
        plan(&e, &input(6), budgets(), &constraints)
            .await
            .unwrap_err()
            .kind,
        ContextErrorKind::CannotFit
    );
}
#[tokio::test]
async fn summary_input_limit_includes_the_actual_instruction_and_wrapper() {
    let e = engine(1000, 0, 0);
    let too_small = CompactionBudgets::new(0, 50, 1);
    assert_eq!(
        plan(&e, &input(3), too_small, &CompactionConstraints::default())
            .await
            .unwrap_err()
            .kind,
        ContextErrorKind::CannotFit
    );
}
#[tokio::test]
async fn candidate_selection_is_deterministic() {
    let e = engine(1000, 0, 200);
    let constraints = CompactionConstraints::default();
    let first = plan(&e, &input(6), budgets(), &constraints).await.unwrap();
    for _ in 0..20 {
        assert_eq!(
            plan(&e, &input(6), budgets(), &constraints).await.unwrap(),
            first
        );
    }
}
#[test]
fn observed_usage_is_reused_only_for_exact_or_unchanged_prefix_input() {
    let e = engine(1000, 0, 0);
    let first = e.build_projection_input(SurfaceRevision::new(2), input(2), &[], None, "fixed");
    let observed = ProviderObservedInput {
        fingerprint: first.fingerprint(),
        input_tokens: 17,
        anchor: Some(ObservedAnchor::of_model_input(
            &first.messages,
            "fixed",
            &[],
        )),
    };
    let exact = e.build_projection_input(
        SurfaceRevision::new(2),
        input(2),
        &[],
        Some(&observed),
        "fixed",
    );
    assert_eq!(exact.estimated_input.input_tokens, 17);
    let extended = e.build_projection_input(
        SurfaceRevision::new(3),
        input(3),
        &[],
        Some(&observed),
        "fixed",
    );
    assert_eq!(extended.estimated_input.input_tokens, 117);
    let changed = e.build_projection_input(
        SurfaceRevision::new(3),
        input(3),
        &[],
        Some(&observed),
        "different",
    );
    assert_ne!(changed.estimated_input.input_tokens, 117);
}
#[test]
fn estimate_does_not_create_provider_usage() {
    let e = engine(1000, 0, 0);
    let projection = e.build_projection_input(SurfaceRevision::new(2), input(2), &[], None, "");
    assert_eq!(
        projection.estimated_input.source,
        rustx::runtime::types::TokenMeasurementSource::Estimated
    );
}
#[test]
fn default_estimator_accepts_identity_free_typed_input() {
    let estimator = DefaultTokenEstimator;
    let original = message(1);
    let derived = ModelInputMessage::Derived(rustx::model::projected::DerivedModelMessage {
        message: rustx::model::projected::ProjectedMessage::from(&original),
        sources: vec![],
        checkpoint_sources: vec![],
        transformations: vec![rustx::model::projected::SemanticTransformation::Summary],
    });
    assert!(estimator.estimate_input(&[derived], "", &[]) > 0);
}

#[tokio::test]
async fn derived_source_mention_cannot_replace_core_fresh_inbound() {
    let original = message(2);
    let fresh =
        rustx::runtime::inbound::FreshInboundTurn::new(vec![original.id().clone()]).unwrap();
    let derived = ModelInputMessage::Derived(rustx::model::projected::DerivedModelMessage {
        message: rustx::model::projected::ProjectedMessage::from(&original),
        sources: vec![rustx::model::projected::CanonicalMessageRef::of(
            &rustx::runtime::identity::ConversationId::generate(),
            &original,
        )],
        checkpoint_sources: vec![],
        transformations: vec![rustx::model::projected::SemanticTransformation::Summary],
    });
    let error = plan(
        &engine(1000, 0, 0),
        &[ModelInputMessage::Canonical(message(1)), derived],
        budgets(),
        &CompactionConstraints {
            fresh_inbound: Some(&fresh),
            ..CompactionConstraints::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ContextErrorKind::MalformedHistory);
}

#[tokio::test]
async fn long_prefix_planning_bounds_actual_rendering_and_complete_estimator_work() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let visits = Arc::new(AtomicUsize::new(0));
    let counted = visits.clone();
    let estimator = rustx::context::ClosureTokenEstimator::new(move |messages, _, _| {
        counted.fetch_add(messages.len(), Ordering::SeqCst);
        messages.len() as u64
    });
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(estimator),
    )
    .unwrap();
    for length in [512, 1024] {
        let history = input(length);
        visits.store(0, Ordering::SeqCst);
        crate::context::summarizer::take_summary_rendered_messages();
        let selected = plan(&e, &history, budgets(), &CompactionConstraints::default())
            .await
            .unwrap();
        assert_eq!(
            crate::context::summarizer::take_summary_rendered_messages(),
            history.len(),
            "the transcript is rendered exactly once"
        );
        assert!(
            visits.load(Ordering::SeqCst)
                <= crate::context::planning::MAX_SUMMARY_CANDIDATES * (2 * history.len() + 1),
            "complete candidate materialization is capped independently of history length"
        );
        assert_eq!(
            plan(&e, &history, budgets(), &CompactionConstraints::default())
                .await
                .unwrap(),
            selected,
            "sampled boundary selection is deterministic"
        );
    }
}

#[tokio::test]
async fn non_additive_planning_checks_complete_requests_and_retention_without_inferred_fit() {
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 200,
            reserve_tokens: 10,
            keep_recent_tokens: 100,
        },
        Arc::new(rustx::context::ClosureTokenEstimator::new(
            |messages, prompt, _| {
                if messages
                    .iter()
                    .any(|m| matches!(m, ModelInputMessage::Derived(_)))
                {
                    return 30;
                }
                // A whole suffix can cost less than each constituent, and adding
                // staged material may make it much more expensive. Neither cost
                // is inferable from singleton prices or monotonicity.
                if prompt == "primary" {
                    if messages.len() == 4 { 50 } else { 500 }
                } else if messages.len() == 3 {
                    100
                } else {
                    0
                }
            },
        )),
    )
    .unwrap();
    let history = input(6);
    let staged = [message(7)];
    let constraints = CompactionConstraints {
        staged_request_context: &staged,
        ..Default::default()
    };
    let n = crate::context::planning::summary_prefix(
        &e,
        &history,
        "primary",
        &[],
        CompactionBudgets::new(10, 20, 30),
        &constraints,
        &rustx::runtime::CancellationSignal::new(),
    )
    .await
    .unwrap();
    assert_eq!(n, 3);
    let full =
        rustx::model::input::assemble_model_input(&history[n..], &staged, None, None).unwrap();
    assert!(e.estimate_model_input(&full, "primary", &[]) + 20 <= e.soft_input_limit(10).unwrap());
    assert!(e.estimate_model_input(&history[n..], "", &[]) >= e.config().keep_recent_tokens);
    let summary = rustx::context::SummaryRequest {
        retired: history[..n].to_vec(),
    }
    .model_input();
    assert!(e.estimate_model_input(&summary.messages, "", &[]) <= 30);
}

#[tokio::test]
async fn incremental_transcript_boundaries_match_complete_summary_requests() {
    let mut history = input(4);
    let ModelInputMessage::Canonical(MessageBlock::User(empty)) = &mut history[1] else {
        unreachable!()
    };
    empty.content.clear();
    let mut rendered = String::new();
    for (index, message) in history.iter().enumerate() {
        crate::context::summarizer::append_summary_message(message, &mut rendered);
        assert_eq!(
            crate::context::summarizer::summary_model_input(&rendered),
            rustx::context::SummaryRequest {
                retired: history[..=index].to_vec()
            }
            .model_input()
        );
    }
}

#[tokio::test]
async fn complete_tool_batch_and_core_owned_boundaries_constrain_every_probe() {
    use rustx::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, ToolCallOccurrenceRef,
        ToolMessageBlock,
    };
    use rustx::runtime::identity::{ToolCallId, ToolId};
    use rustx::tools::types::{ToolCall, ToolExecutionResult};
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("calls"),
        content: ["a", "b"]
            .map(|id| {
                AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new(id),
                    tool_id: ToolId::new("tool-read"),
                    name: "read".into(),
                    arguments: serde_json::json!({}),
                })
            })
            .into(),
    });
    let results = ["b", "a"].into_iter().enumerate().map(|(index, id)| {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(format!("result-{id}")),
            occurrence: ToolCallOccurrenceRef::new(
                MessageId::new("calls"),
                ContentBlockIndex::new(u32::try_from(1 - index).unwrap()),
            ),
            tool_call_id: ToolCallId::new(id),
            tool_id: ToolId::new("tool-read"),
            result: ToolExecutionResult {
                status: rustx::tools::types::ToolExecutionStatus::Success,
                content: vec![],
                duration_ms: 0,
                exit_code: None,
                artifacts: vec![],
                deliveries: vec![],
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        })
    });
    let mut history = vec![message(1), assistant];
    history.extend(results);
    history.extend((5..=8).map(message));
    let history = canonical_input(&history);
    let cover = MessageId::new("calls");
    let fresh = rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("M7")]).unwrap();
    let protected = [MessageId::new("M6")];
    let anchor = rustx::model::RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new("M5"));
    let e = engine(10_000, 0, 0);
    let constraints = CompactionConstraints {
        must_cover_through: Some(&cover),
        fresh_inbound: Some(&fresh),
        protected_context_ids: &protected,
        carryover_anchor: Some(&anchor),
        ..Default::default()
    };
    assert_eq!(
        plan(&e, &history, budgets(), &constraints).await.unwrap(),
        4
    );
    for retained in ["M5", "M6", "M7"] {
        assert!(history[4..].iter().any(|item| {
            item.canonical_id()
                .is_some_and(|id| id.as_str() == retained)
        }));
    }
    let split = rustx::model::RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new("result-a"));
    assert_eq!(
        plan(
            &e,
            &history,
            budgets(),
            &CompactionConstraints {
                carryover_anchor: Some(&split),
                ..constraints
            }
        )
        .await
        .unwrap_err()
        .kind,
        ContextErrorKind::CannotFit
    );
}
