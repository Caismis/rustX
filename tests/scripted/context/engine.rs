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
    assert!(
        e.fits_under_soft_limit(&p, 0).unwrap(),
        "hard admission accepts equality"
    );
    for (tokens, triggered) in [(179, false), (180, true), (181, true)] {
        let estimator = ScriptedEstimator::new(100, 100, 0).with_override("M1", tokens);
        let e = ContextEngine::new(
            ContextConfig {
                context_window_tokens: 200,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(estimator),
        )
        .unwrap();
        let p = e.build_projection_input(SurfaceRevision::new(1), input(1).into(), &[], None, "");
        assert_eq!(e.should_compact(&p, 0).unwrap(), triggered);
        assert!(e.fits_under_soft_limit(&p, 0).unwrap());
    }
}
#[test]
fn soft_limit_respects_output_budget_and_reserve() {
    let e = engine(1000, 100, 0);
    assert_eq!(e.admissible_input_limit(200).unwrap(), 700);
    assert!(e.admissible_input_limit(900).is_err());
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
    let history = input(6);
    let selected = plan(&e, &history, budgets(), &CompactionConstraints::default())
        .await
        .unwrap();
    let candidate = |count| -> rustx::model::SelectedModelInput {
        let mut messages = vec![crate::context::planning::minimum_summary()];
        messages.extend_from_slice(&history[count..]);
        messages.into()
    };
    assert_eq!(selected, 4);
    assert!(e.estimate_model_input(&candidate(selected), "", &[]) <= 250);
    assert!(e.estimate_model_input(&candidate(selected - 1), "", &[]) > 250);
    assert_eq!(
        e.estimate_model_input(&history[selected..].to_vec().into(), "", &[]),
        200,
        "the largest admissible retained suffix is below the soft target"
    );
}
#[tokio::test]
async fn summary_output_tokens_do_not_change_primary_retirement() {
    let e = engine(500, 0, 0);
    let low = CompactionBudgets::new(100, 50, 100_000);
    let high = CompactionBudgets::new(100, 300, 100_000);
    let first = plan(&e, &input(6), low, &CompactionConstraints::default())
        .await
        .unwrap();
    assert_eq!(
        plan(&e, &input(6), high, &CompactionConstraints::default())
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        plan(
            &e,
            &input(6),
            CompactionBudgets::new(100, 500, 100_000),
            &CompactionConstraints::default()
        )
        .await
        .unwrap(),
        first
    );
}

#[tokio::test]
async fn primary_prices_real_minimum_summary_with_protected_input() {
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        messages
            .iter()
            .map(|message| match message {
                ModelInputMessage::Derived(_) => 50,
                ModelInputMessage::Canonical(block) if block.id().as_str() == "M1" => 2_000,
                ModelInputMessage::Canonical(_) => 900,
                ModelInputMessage::RequestOnly(_) => 0,
            })
            .sum()
    });
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 1_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(estimator),
    )
    .unwrap();
    let fresh = rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("M2")]).unwrap();
    let constraints = CompactionConstraints {
        fresh_inbound: Some(&fresh),
        ..Default::default()
    };
    assert_eq!(
        plan(
            &e,
            &input(2),
            CompactionBudgets::new(0, 250, 10_000),
            &constraints
        )
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn default_planner_uses_shortest_valid_summary_at_protected_input_boundary() {
    let mut old = message(1);
    let MessageBlock::User(old_user) = &mut old else {
        unreachable!()
    };
    old_user.content = vec![UserContentBlock::Text(TextBlock {
        text: "old source ".repeat(500),
    })];
    let history = canonical_input(&[old, message(2)]);
    let minimum = crate::context::planning::minimum_summary();
    let mut empty_sections = minimum.clone();
    let ModelInputMessage::Derived(derived) = &mut empty_sections else {
        unreachable!()
    };
    let rustx::model::projected::ProjectedMessage::User { content, .. } = &mut derived.message
    else {
        unreachable!()
    };
    let UserContentBlock::Text(text) = &mut content[0] else {
        unreachable!()
    };
    text.text = crate::context::summary_content::render(serde_json::json!({
        "goal": "x",
        "constraints_and_preferences": [],
        "progress": {"done": [], "in_progress": [], "blocked": []},
        "key_decisions": [],
        "next_steps": [],
        "critical_context": []
    }))
    .unwrap()
    .trim_end()
    .to_owned();
    let candidate = |summary| vec![summary, history[1].clone()];
    let limit = DefaultTokenEstimator.estimate_input(&candidate(minimum), "", &[]);
    assert!(DefaultTokenEstimator.estimate_input(&candidate(empty_sections), "", &[]) > limit);
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: limit,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(DefaultTokenEstimator),
    )
    .unwrap();
    let fresh = rustx::runtime::inbound::FreshInboundTurn::new(vec![MessageId::new("M2")]).unwrap();
    let constraints = CompactionConstraints {
        fresh_inbound: Some(&fresh),
        ..CompactionConstraints::default()
    };
    assert_eq!(
        plan(&e, &history, budgets(), &constraints).await.unwrap(),
        1
    );
    let too_small = ContextEngine::new(
        ContextConfig {
            context_window_tokens: limit - 1,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(DefaultTokenEstimator),
    )
    .unwrap();
    assert_eq!(
        plan(&too_small, &history, budgets(), &constraints)
            .await
            .unwrap_err()
            .kind,
        ContextErrorKind::CannotFit
    );
}

#[tokio::test]
async fn arbitrary_summary_price_without_progress_witness_is_not_proven_impossible() {
    let estimator = Arc::new(rustx::context::ClosureTokenEstimator::new(
        |messages, _, _| {
            if messages.iter().any(|message| {
                matches!(message, ModelInputMessage::Derived(derived)
                if serde_json::to_string(&derived.message).unwrap().contains("alternative"))
            }) {
                1
            } else {
                100
            }
        },
    ));
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 1_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator.clone(),
    )
    .unwrap();
    let history = input(1);
    let mut alternative = crate::context::planning::minimum_summary();
    let ModelInputMessage::Derived(derived) = &mut alternative else {
        unreachable!()
    };
    let rustx::model::projected::ProjectedMessage::User { content, .. } = &mut derived.message
    else {
        unreachable!()
    };
    let UserContentBlock::Text(text) = &mut content[0] else {
        unreachable!()
    };
    text.text = text.text.replacen("- x", "- alternative", 1);
    assert!(
        e.estimate_model_input(&vec![alternative].into(), "", &[])
            < e.estimate_model_input(&history.clone().into(), "", &[])
    );
    let error = plan(&e, &history, budgets(), &CompactionConstraints::default())
        .await
        .unwrap_err();
    assert_eq!(error.kind, ContextErrorKind::SearchIncomplete);
    assert!(error.message.contains("synthetic Summary candidate"));
    let no_fit = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 50,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .unwrap();
    let error = plan(
        &no_fit,
        &history,
        budgets(),
        &CompactionConstraints::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ContextErrorKind::SearchIncomplete);
}

#[tokio::test]
async fn certified_summary_floor_can_prove_no_historical_progress() {
    let e = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 1_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(DefaultTokenEstimator),
    )
    .unwrap();
    let error = plan(&e, &input(1), budgets(), &CompactionConstraints::default())
        .await
        .unwrap_err();
    assert_eq!(error.kind, ContextErrorKind::NoProgress);
}

#[tokio::test]
async fn selected_prefix_must_include_new_source_after_existing_summary() {
    let history = vec![
        crate::context::planning::minimum_summary(),
        input(1).remove(0),
    ];
    let e = engine(1_000, 0, 100);
    assert_eq!(
        plan(&e, &history, budgets(), &CompactionConstraints::default())
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        plan(
            &e,
            &history[..1],
            budgets(),
            &CompactionConstraints::default()
        )
        .await
        .unwrap_err()
        .kind,
        ContextErrorKind::NoEligibleHistory
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
        ContextErrorKind::NoEligibleHistory
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
        ContextErrorKind::MalformedHistory
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
    let error = plan(&e, &input(6), budgets(), &constraints)
        .await
        .unwrap_err();
    assert_eq!(error.kind, ContextErrorKind::SearchIncomplete);
    assert!(
        error
            .message
            .contains("protected Fresh Inbound/current Context=500")
    );
    assert!(error.message.contains("not additive or provider usage"));
}

#[tokio::test]
async fn fit_failure_labels_fixed_input_without_treating_components_as_additive() {
    let prompt = "x".repeat(500);
    let e = engine(100, 0, 0);
    let error = crate::context::planning::summary_prefix(
        &e,
        &input(1).into(),
        &prompt,
        &[],
        budgets(),
        &CompactionConstraints::default(),
        &rustx::runtime::CancellationSignal::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ContextErrorKind::SearchIncomplete);
    assert!(error.message.contains("fixed System Prompt/Tools=125"));
    assert!(
        error
            .message
            .contains("protected Fresh Inbound/current Context=not present")
    );
    assert!(error.message.contains("complete candidate estimate="));

    // A nonmonotone estimator can price a combined request below either
    // component; component-only estimates must never reject this candidate.
    let arbitrary = ContextEngine::new(
        ContextConfig {
            context_window_tokens: 100,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(rustx::context::ClosureTokenEstimator::new(
            |messages, _, _| {
                if messages
                    .iter()
                    .any(|message| matches!(message, ModelInputMessage::Derived(_)))
                {
                    50
                } else {
                    500
                }
            },
        )),
    )
    .unwrap();
    assert_eq!(
        crate::context::planning::summary_prefix(
            &arbitrary,
            &input(1).into(),
            &prompt,
            &[],
            budgets(),
            &CompactionConstraints::default(),
            &rustx::runtime::CancellationSignal::new(),
        )
        .await
        .unwrap(),
        1
    );
}
#[tokio::test]
async fn retirement_selection_is_independent_of_summary_input_capacity() {
    let e = engine(1000, 0, 0);
    let too_small = CompactionBudgets::new(0, 50, 1);
    assert_eq!(
        plan(&e, &input(3), too_small, &CompactionConstraints::default())
            .await
            .unwrap(),
        3
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
            0,
            "primary retirement does not render summary source material"
        );
        assert!(
            visits.load(Ordering::SeqCst)
                <= history.len()
                    + crate::context::planning::MAX_GENERIC_CANDIDATES * (3 * history.len() + 3),
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
                // A whole suffix can cost less than each constituent, and adding
                // staged material may make it much more expensive. Neither cost
                // is inferable from singleton prices or monotonicity.
                if prompt == "primary" {
                    if messages.len() == 5 { 50 } else { 500 }
                } else if messages.len() == 6
                    && messages
                        .iter()
                        .all(|message| matches!(message, ModelInputMessage::Canonical(_)))
                {
                    200
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
    let mut full = vec![crate::context::planning::minimum_summary()];
    full.extend_from_slice(&history[n..]);
    let full = rustx::model::input::assemble_model_input(&full, &staged, None, None).unwrap();
    assert!(
        e.estimate_model_input(&full.into(), "primary", &[])
            <= e.admissible_input_limit(10).unwrap()
    );
    assert!(
        e.estimate_model_input(&history[n..].to_vec().into(), "", &[])
            >= e.config().keep_recent_tokens
    );
}

#[tokio::test]
async fn detached_fragment_covers_complete_utf8_source_ranges() {
    let mut history = input(4);
    let ModelInputMessage::Canonical(MessageBlock::User(first)) = &mut history[0] else {
        unreachable!()
    };
    first.content = vec![UserContentBlock::Text(TextBlock {
        text: "Unicode 雪🙂".into(),
    })];
    let ModelInputMessage::Canonical(MessageBlock::User(empty)) = &mut history[1] else {
        unreachable!()
    };
    empty.content.clear();
    let mut plan =
        crate::context::chunks::ChunkPlan::new(&history.clone().into(), history.len()).unwrap();
    let request = plan
        .next(None, None, Some(100), 100, true, |_| Ok(1))
        .unwrap();
    let fragment = request.fragment.as_ref().expect("detached fragment");
    assert_eq!(fragment.ranges.len(), history.len());
    let mut pieces = Vec::new();
    for (index, (message, range)) in history.iter().zip(&fragment.ranges).enumerate() {
        let mut rendered = String::new();
        crate::context::summarizer::append_summary_message(message, &mut rendered);
        assert_eq!(range.projection_index as usize, index);
        assert_eq!(
            range.role,
            rustx::model::auxiliary::AuxiliarySourceRole::User
        );
        assert_eq!(range.byte_start, 0);
        assert_eq!(range.byte_end as usize, rendered.len());
        if !rendered.is_empty() {
            pieces.push(rendered);
        }
    }
    assert_eq!(fragment.text, pieces.join("\n\n"));
    assert!(request.render_transcript().contains("Unicode 雪🙂"));
    plan.accept(&request).unwrap();
    assert!(plan.is_complete());
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
            ContextErrorKind::NoEligibleHistory
        );
    }
}

#[tokio::test]
async fn n1_unsampled_only_feasible_boundary_is_not_proven_cannot_fit() {
    let history = input(129);
    let estimator = rustx::context::ClosureTokenEstimator::new(|messages, _, _| {
        if matches!(messages.first(), Some(ModelInputMessage::Derived(_))) && messages.len() == 128
        {
            1
        } else {
            1_000
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
    assert_eq!(
        outcome.unwrap_err().kind,
        ContextErrorKind::SearchIncomplete
    );
}

#[tokio::test]
async fn n1_default_search_matches_exhaustive_primary_request_prices() {
    let history = input(129);
    let primary_cost = |n| {
        let mut candidate = vec![crate::context::planning::minimum_summary()];
        candidate.extend_from_slice(&history[n..]);
        DefaultTokenEstimator.estimate_input(&candidate, "", &[])
    };
    let tail_cost = |n| DefaultTokenEstimator.estimate_input(&history[n..], "", &[]);
    for (first, recent) in [(1, 0), (129, 0), (2, 0), (2, tail_cost(50)), (2, u64::MAX)] {
        let e = ContextEngine::new(
            ContextConfig {
                context_window_tokens: primary_cost(first),
                reserve_tokens: 0,
                keep_recent_tokens: recent,
            },
            Arc::new(DefaultTokenEstimator),
        )
        .unwrap();
        let history_before = DefaultTokenEstimator.estimate_input(&history, "", &[]);
        let feasible: Vec<_> = (1..=129)
            .filter(|n| {
                primary_cost(*n) <= e.admissible_input_limit(0).unwrap()
                    && primary_cost(*n) < history_before
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
                CompactionBudgets::new(0, 50, 1),
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
        if [29, 24, 12].contains(&messages.len()) {
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
        ContextErrorKind::SearchIncomplete
    );
}

#[tokio::test]
async fn n1_default_search_bounds_actual_render_copy_and_serialization_work() {
    for length in [512, 1024] {
        let history = input(length);
        let logical_bytes = serde_json::to_vec(&history).unwrap().len() as u64;
        let summary_bytes = serde_json::to_vec(&crate::context::planning::minimum_summary())
            .unwrap()
            .len() as u64;
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
            0,
            "retirement search never renders the historical summary input"
        );
        assert!(crate::context::tokens::take_estimator_calls() <= 3 * probes);
        assert_eq!(crate::context::summarizer::take_summary_copied_bytes(), 0);
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
        let logical_bytes = serde_json::to_vec(&history).unwrap().len();
        let summary_bytes = serde_json::to_vec(&crate::context::planning::minimum_summary())
            .unwrap()
            .len();
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
            0,
            "media-aware retirement search does not render summary fragments"
        );
        assert!(
            usize::try_from(crate::context::tokens::take_estimator_calls()).unwrap() <= 3 * probes
        );
        assert!(work.projections <= 9 * probes + 3);
        assert!(work.copied_bytes <= (9 * probes + 3) * (summary_bytes + 2 * logical_bytes));
    }
}
