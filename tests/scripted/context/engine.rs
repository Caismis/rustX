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
        &input.to_vec().into(),
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
    let p = e.build_projection_input(SurfaceRevision::new(1), input(1).into(), &[], None, "");
    assert!(!e.should_compact(&p, 0).unwrap());
}
#[test]
fn threshold_equality_compacts() {
    let e = engine(200, 0, 0);
    let p = e.build_projection_input(SurfaceRevision::new(2), input(2).into(), &[], None, "");
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
    let a = e.build_projection_input(SurfaceRevision::new(3), input(3).into(), &[], None, "fixed");
    let b = e.build_projection_input(SurfaceRevision::new(3), input(3).into(), &[], None, "fixed");
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
    let first =
        e.build_projection_input(SurfaceRevision::new(2), input(2).into(), &[], None, "fixed");
    let observed = ProviderObservedInput {
        input_tokens: 17,
        anchor: ObservedAnchor::of_selected(first.surface_revision, &first.messages, "fixed", &[]),
    };
    let exact = e.build_projection_input(
        SurfaceRevision::new(2),
        input(2).into(),
        &[],
        Some(&observed),
        "fixed",
    );
    assert_eq!(exact.estimated_input.input_tokens, 17);
    let extended = e.build_projection_input(
        SurfaceRevision::new(3),
        input(3).into(),
        &[],
        Some(&observed),
        "fixed",
    );
    assert_eq!(extended.estimated_input.input_tokens, 117);
    let changed = e.build_projection_input(
        SurfaceRevision::new(3),
        input(3).into(),
        &[],
        Some(&observed),
        "different",
    );
    assert_ne!(changed.estimated_input.input_tokens, 117);
}
#[test]
fn estimate_does_not_create_provider_usage() {
    let e = engine(1000, 0, 0);
    let projection =
        e.build_projection_input(SurfaceRevision::new(2), input(2).into(), &[], None, "");
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
                <= crate::context::planning::MAX_GENERIC_CANDIDATES * (2 * history.len() + 1),
            "complete candidate materialization is capped independently of history length"
        );
        assert_eq!(
            plan(&e, &history, budgets(), &CompactionConstraints::default())
                .await
                .unwrap(),
            selected,
            "bounded boundary selection is deterministic"
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
        &history.clone().into(),
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
    assert!(
        e.estimate_model_input(&full.into(), "primary", &[]) + 20
            <= e.soft_input_limit(10).unwrap()
    );
    assert!(
        e.estimate_model_input(&history[n..].to_vec().into(), "", &[])
            >= e.config().keep_recent_tokens
    );
    let summary = rustx::context::SummaryRequest {
        retired: history[..n].to_vec(),
    }
    .model_input();
    assert!(e.estimate_model_input(&summary.messages.into(), "", &[]) <= 30);
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
    for with_media in [false, true] {
        let mut history = vec![message(1), assistant.clone()];
        history.extend(results.clone());
        history.extend((5..=8).map(message));
        let root = tempfile::tempdir().unwrap();
        let artifacts = rustx::tools::artifacts::ArtifactStore::new(
            rustx::runtime::ConversationId::generate(),
            root.path(),
        )
        .unwrap();
        if with_media {
            let mut bytes = Vec::new();
            png::Encoder::new(&mut bytes, 1, 1)
                .write_header()
                .unwrap()
                .write_image_data(&[0])
                .unwrap();
            let MessageBlock::User(first) = &mut history[0] else {
                unreachable!()
            };
            first.content.push(UserContentBlock::Image(
                rustx::message::content::ImageReference {
                    artifact_id: artifacts.put_bounded(&bytes).unwrap(),
                    alt: None,
                },
            ));
        }
        let history = canonical_input(&history);
        let cover = MessageId::new("calls");
        let fresh =
            rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("M7")]).unwrap();
        let protected = [MessageId::new("M6")];
        let anchor = rustx::model::RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new("M5"));
        let mut e = if with_media {
            ContextEngine::new(
                ContextConfig {
                    context_window_tokens: 10_000,
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                },
                Arc::new(DefaultTokenEstimator),
            )
            .unwrap()
        } else {
            engine(10_000, 0, 0)
        };
        if with_media {
            e.set_image_projection(artifacts, true);
            e.prepare_media(
                &history.clone().into(),
                &history.clone().into(),
                &rustx::runtime::CancellationSignal::new(),
            )
            .await
            .unwrap();
            assert!(e.supports_monotone_compaction(&history.clone().into()));
        }
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
        let split =
            rustx::model::RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new("result-a"));
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
}

#[tokio::test]
async fn n1_unsampled_only_feasible_boundary_is_not_proven_cannot_fit() {
    let history = input(129);
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        if let [ModelInputMessage::Derived(derived)] = messages {
            let rustx::model::projected::ProjectedMessage::User { content, .. } = &derived.message
            else {
                unreachable!()
            };
            let UserContentBlock::Text(text) = &content[0] else {
                unreachable!()
            };
            if text.text.contains("message 3\n") {
                1000
            } else {
                1
            }
        } else if messages.len() >= 128 {
            1000
        } else {
            1
        }
    });
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(estimator),
    )
    .unwrap();
    let outcome = plan(
        &e,
        &history,
        CompactionBudgets::new(0, 1, 10),
        &CompactionConstraints::default(),
    )
    .await;
    match outcome {
        Ok(n) => assert_eq!(n, 2),
        Err(error) => assert_eq!(
            error.kind,
            ContextErrorKind::SearchIncomplete,
            "boundary 2 is feasible; not examining it proves nothing"
        ),
    }
}

#[tokio::test]
async fn n1_default_estimator_finds_only_feasible_second_of_129_boundaries() {
    let history = input(129);
    let summary_cost = |n| {
        DefaultTokenEstimator.estimate_input(
            &rustx::context::SummaryRequest {
                retired: history[..n].to_vec(),
            }
            .model_input()
            .messages,
            "",
            &[],
        )
    };
    let tail_cost = |n| DefaultTokenEstimator.estimate_input(&history[n..], "", &[]);
    assert!(summary_cost(3) > summary_cost(2));
    assert!(tail_cost(1) > tail_cost(2));
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: tail_cost(2) + 50,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(DefaultTokenEstimator),
    )
    .unwrap();
    assert_eq!(
        plan(
            &e,
            &history,
            CompactionBudgets::new(0, 50, summary_cost(2)),
            &CompactionConstraints::default()
        )
        .await
        .unwrap(),
        2
    );
}

#[tokio::test]
async fn n1_default_search_preserves_extremes_preference_and_fallback() {
    let history = input(129);
    let summary_cost = |n| {
        DefaultTokenEstimator.estimate_input(
            &rustx::context::SummaryRequest {
                retired: history[..n].to_vec(),
            }
            .model_input()
            .messages,
            "",
            &[],
        )
    };
    let tail_cost = |n| DefaultTokenEstimator.estimate_input(&history[n..], "", &[]);
    for (first, last, recent) in [
        (1, 1, 0),
        (129, 129, 0),
        (2, 100, 0),
        (2, 100, tail_cost(50)),
        (2, 100, u64::MAX),
    ] {
        let e = ContextEngine::new(
            ContextConfig {
                context_window_tokens: tail_cost(first) + 50,
                reserve_tokens: 0,
                keep_recent_tokens: recent,
            },
            Arc::new(DefaultTokenEstimator),
        )
        .unwrap();
        let feasible: Vec<_> = (1..=129)
            .filter(|n| {
                summary_cost(*n) <= summary_cost(last)
                    && tail_cost(*n) + 50 <= e.soft_input_limit(0).unwrap()
            })
            .collect();
        let expected = feasible
            .iter()
            .copied()
            .filter(|n| tail_cost(*n) >= recent)
            .max()
            .unwrap_or(feasible[0]);
        assert_eq!(
            plan(
                &e,
                &history,
                CompactionBudgets::new(0, 50, summary_cost(last)),
                &CompactionConstraints::default()
            )
            .await
            .unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn n1_arbitrary_non_monotone_prices_require_actual_candidate_queries() {
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        if matches!(messages, [ModelInputMessage::Derived(_)])
            || [28, 23, 11].contains(&messages.len())
        {
            1
        } else {
            1000
        }
    });
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(estimator),
    )
    .unwrap();
    assert_eq!(
        plan(
            &e,
            &input(30),
            CompactionBudgets::new(0, 1, 10),
            &CompactionConstraints::default()
        )
        .await
        .unwrap(),
        19
    );
}

#[tokio::test]
async fn n1_generic_exhaustion_distinguishes_incomplete_search_from_infeasibility() {
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(rustx::context::ClosureTokenEstimator::new(|_, _, _| 1000)),
    )
    .unwrap();
    let incomplete = plan(
        &e,
        &input(129),
        CompactionBudgets::new(0, 1, 10),
        &CompactionConstraints::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(incomplete.kind, ContextErrorKind::SearchIncomplete);
    assert_eq!(
        incomplete.message,
        "compaction candidate search incomplete: inspected 64 of 129 eligible boundaries; estimator has no monotone accounting guarantee"
    );
    assert_eq!(
        plan(
            &e,
            &input(64),
            CompactionBudgets::new(0, 1, 10),
            &CompactionConstraints::default()
        )
        .await
        .unwrap_err()
        .kind,
        ContextErrorKind::CannotFit
    );
}

#[tokio::test]
async fn n1_default_search_bounds_actual_render_copy_and_serialization_work() {
    for length in [512, 1024] {
        let history = input(length);
        let full_summary = rustx::context::SummaryRequest {
            retired: history.clone(),
        }
        .model_input();
        let logical_bytes = serde_json::to_vec(&history).unwrap().len() as u64;
        let summary_bytes = serde_json::to_vec(&full_summary.messages).unwrap().len() as u64;
        let e = ContextEngine::new(
            ContextConfig {
                context_window_tokens: 100_000,
                reserve_tokens: 0,
                keep_recent_tokens: 100,
            },
            Arc::new(DefaultTokenEstimator),
        )
        .unwrap();
        crate::context::summarizer::take_summary_rendered_messages();
        crate::context::summarizer::take_summary_copied_bytes();
        crate::context::planning::take_copied_bytes();
        crate::context::tokens::take_accounted_bytes();
        crate::context::tokens::take_estimator_calls();
        plan(&e, &history, budgets(), &CompactionConstraints::default())
            .await
            .unwrap();
        let probes = 3 * (u64::from(length.ilog2()) + 1) + 1;
        assert_eq!(
            crate::context::summarizer::take_summary_rendered_messages(),
            length
        );
        assert!(crate::context::tokens::take_estimator_calls() <= 3 * probes);
        assert!(crate::context::summarizer::take_summary_copied_bytes() <= probes * summary_bytes);
        assert!(crate::context::planning::take_copied_bytes() <= probes * logical_bytes);
        assert!(
            crate::context::tokens::take_accounted_bytes()
                <= probes * (summary_bytes + 2 * logical_bytes + 100)
        );
    }
}

#[tokio::test]
async fn production_media_search_bounds_reads_renders_queries_and_copied_bytes() {
    use rustx::message::content::{ImageReference, UploadedFileRef};
    use rustx::model::uploads::{ResolvedUpload, UploadProjection, UploadProjectionResolver};
    use rustx::runtime::ConversationId;
    use rustx::tools::artifacts::ArtifactStore;
    #[derive(Debug)]
    struct Owner;
    impl UploadProjectionResolver for Owner {
        fn resolve(&self, input: &[ModelInputMessage]) -> Result<UploadProjection, String> {
            let mut refs = std::collections::BTreeSet::new();
            for m in input {
                if let rustx::model::projected::ModelMessageView::User(c) = m.view() {
                    for b in c {
                        if let UserContentBlock::UploadedFile(f) = b {
                            refs.insert(f.clone());
                        }
                    }
                }
            }
            Ok(UploadProjection {
                files: refs
                    .into_iter()
                    .map(|file| ResolvedUpload {
                        file,
                        path: "/session/file.txt".into(),
                    })
                    .collect(),
            })
        }
    }
    for length in [512, 1024] {
        let root = tempfile::tempdir().unwrap();
        let artifacts = ArtifactStore::new(ConversationId::generate(), root.path()).unwrap();
        let mut bytes = Vec::new();
        png::Encoder::new(&mut bytes, 1, 1)
            .write_header()
            .unwrap()
            .write_image_data(&[0])
            .unwrap();
        let id = artifacts.put_bounded(&bytes).unwrap();
        let mut history = input(length);
        for m in &mut history {
            let ModelInputMessage::Canonical(MessageBlock::User(u)) = m else {
                unreachable!()
            };
            u.content.extend([
                UserContentBlock::Image(ImageReference {
                    artifact_id: id.clone(),
                    alt: None,
                }),
                UserContentBlock::UploadedFile(UploadedFileRef {
                    batch_id: "batch".into(),
                    name: "file.txt".into(),
                }),
            ]);
        }
        let mut e = ContextEngine::new(
            ContextConfig {
                context_window_tokens: 100_000,
                reserve_tokens: 0,
                keep_recent_tokens: 100,
            },
            Arc::new(DefaultTokenEstimator),
        )
        .unwrap();
        e.set_image_projection(artifacts, true);
        e.set_upload_resolver(Arc::new(Owner));
        crate::model::media::take_work();
        e.prepare_media(
            &history.clone().into(),
            &history.clone().into(),
            &rustx::runtime::CancellationSignal::new(),
        )
        .await
        .unwrap();
        let captured = crate::model::media::take_work();
        assert_eq!(captured.reads, 1);
        assert_eq!(captured.resolutions, 1);
        let full_summary = rustx::context::SummaryRequest {
            retired: history.clone(),
        }
        .model_input();
        let logical_bytes = serde_json::to_vec(&history).unwrap().len();
        let summary_bytes = serde_json::to_vec(&full_summary.messages).unwrap().len();
        crate::context::summarizer::take_summary_rendered_messages();
        crate::context::tokens::take_estimator_calls();
        plan(
            &e,
            &history,
            CompactionBudgets::new(0, 50, 1_000_000),
            &CompactionConstraints::default(),
        )
        .await
        .unwrap();
        let probes = 3 * (length.ilog2() as usize + 1) + 1;
        let work = crate::model::media::take_work();
        assert_eq!(work.reads, 0);
        assert_eq!(work.resolutions, 0);
        assert_eq!(
            crate::context::summarizer::take_summary_rendered_messages(),
            length
        );
        assert!(
            usize::try_from(crate::context::tokens::take_estimator_calls()).unwrap() <= 3 * probes
        );
        assert!(work.projections <= 9 * probes + 3);
        assert!(work.copied_bytes <= (9 * probes + 3) * (summary_bytes + 2 * logical_bytes));
    }
}
