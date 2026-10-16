use super::*;
use crate::context::{ContextConfig, ContextEngine};
use crate::message::content::TextBlock;
use crate::message::types::{InboundKind, UserContentBlock, UserMessageBlock, UserSource};
use crate::model::SelectedModelInput;
use crate::model::projected::{DerivedModelMessage, ProjectedMessage, SemanticTransformation};
use crate::runtime::identity::{MessageId, ToolId};
use crate::runtime::types::TokenMeasurementSource;
use std::sync::Arc;

fn user(index: usize, text: &str) -> ModelInputMessage {
    ModelInputMessage::Canonical(MessageBlock::User(UserMessageBlock {
        id: MessageId::new(format!("measured-{index}")),
        source: UserSource::Human,
        kind: InboundKind::Message,
        content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
        timestamp: None,
    }))
}
fn derived(text: &str) -> ModelInputMessage {
    ModelInputMessage::Derived(DerivedModelMessage {
        message: ProjectedMessage::from(user(0, text).as_canonical().unwrap()),
        sources: Vec::new(),
        checkpoint_sources: Vec::new(),
        transformations: vec![SemanticTransformation::Summary],
    })
}
fn request_only() -> ModelInputMessage {
    ModelInputMessage::RequestOnly(
        crate::model::RequestOnlyModelContext::UnresolvedOutputCarryover(
            crate::model::RenderedUnresolvedOutputCarryover {
                source_stream_id: crate::runtime::identity::PublicationStreamId::new(
                    "measured-original-stream",
                ),
                source_settlement: crate::model::UnresolvedOutputSettlement::Incomplete,
                records: Vec::new(),
                omitted_blocks: crate::model::CarryoverOmissionCounts::default(),
            },
        ),
    )
}
fn tools() -> Vec<ModelToolDefinition> {
    vec![ModelToolDefinition {
        id: ToolId::new("measured-tool"),
        name: "measured".into(),
        description: "exact 水 \\\" JSON".into(),
        input_schema: serde_json::json!({"type":"object","properties":{"opaque":{"enum":[-0.0,18_446_744_073_709_551_615_u64,"\\n"]}}}),
    }]
}
fn engine(estimator: Arc<dyn TokenEstimator>) -> ContextEngine {
    ContextEngine::new(
        ContextConfig {
            context_window_tokens: 1_000_000,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        estimator,
    )
    .unwrap()
}

#[test]
fn shared_default_measurement_preserves_flat_json_bytes_rounding_and_legacy_fingerprint() {
    let tools = tools();
    for length in 0..65 {
        let flat = vec![
            user(1, &"x".repeat(length)),
            derived("水🙂 \" \\\n"),
            user(1, "repeated occurrence"),
            request_only(),
        ];
        let selected: SelectedModelInput = flat.clone().into();
        let expected = DefaultTokenEstimator.estimate_input(&flat, "prompt\n水", &tools);
        assert_eq!(
            DefaultTokenEstimator.estimate_selected(&selected, "prompt\n水", &tools),
            expected
        );
        let projection = engine(Arc::new(DefaultTokenEstimator)).build_projection_input(
            SurfaceRevision::new(7),
            selected,
            &tools,
            None,
            "prompt\n水",
        );
        assert_eq!(projection.estimated_input.input_tokens, expected);
        assert_eq!(
            projection.fingerprint(),
            model_input_fingerprint(SurfaceRevision::new(7), &flat, "prompt\n水")
        );
    }
    let empty = SelectedModelInput::default();
    assert_eq!(DefaultTokenEstimator.estimate_selected(&empty, "", &[]), 1);
}

#[test]
fn hypothetical_selected_assembly_preserves_flat_order_and_shares_the_historical_prefix() {
    let flat: Vec<_> = (0..300).map(|index| user(index, "historical")).collect();
    let selected: SelectedModelInput = flat.clone().into();
    let staged = vec![user(300, "new context").as_canonical().unwrap().clone()];
    let ModelInputMessage::RequestOnly(
        crate::model::RequestOnlyModelContext::UnresolvedOutputCarryover(carryover),
    ) = request_only()
    else {
        unreachable!("request-only fixture")
    };
    for anchor in [
        crate::model::RequestOnlyInsertionAnchor::AfterCanonical,
        crate::model::RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new("measured-7")),
        crate::model::RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new("measured-300")),
    ] {
        let expected = crate::model::input::assemble_model_input(
            &flat,
            &staged,
            Some(&carryover),
            Some(&anchor),
        )
        .unwrap();
        let actual = crate::context::engine::assemble_context_input(
            &selected,
            &staged,
            Some(&carryover),
            Some(&anchor),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
    let appended =
        crate::context::engine::assemble_context_input(&selected, &staged, None, None).unwrap();
    assert!(appended.range(0..selected.len()).same_input(&selected));
    assert_eq!(selected.to_vec(), flat);
    assert!(
        crate::context::engine::assemble_context_input(&selected, &staged, Some(&carryover), None,)
            .is_err()
    );
    let absent = crate::model::RequestOnlyInsertionAnchor::BeforeMessage(MessageId::new("missing"));
    assert_eq!(
        crate::context::engine::assemble_context_input(
            &selected,
            &staged,
            Some(&carryover),
            Some(&absent)
        )
        .unwrap_err(),
        crate::model::input::assemble_model_input(&flat, &staged, Some(&carryover), Some(&absent))
            .unwrap_err(),
    );
}

#[test]
fn observed_shared_root_uses_exact_content_and_scalars_across_distinct_roots_and_edits() {
    let flat = vec![
        user(1, "opening"),
        derived("summary"),
        user(1, "duplicate identity"),
        request_only(),
    ];
    let selected: SelectedModelInput = flat.clone().into();
    let tools = tools();
    let anchor = ObservedAnchor::of_selected(SurfaceRevision::new(3), &selected, "fixed", &tools);
    let distinct: SelectedModelInput = flat.clone().into();
    assert!(anchor.matches_selected(SurfaceRevision::new(3), &distinct, "fixed", &tools));
    let mut extended = distinct.clone();
    extended.push(user(4, "tail"));
    assert_eq!(
        anchor.covered_prefix_model_input(&extended, "fixed", &tools),
        Some(4)
    );
    assert!(!anchor.matches_selected(SurfaceRevision::new(4), &extended, "fixed", &tools));
    let mut changed = extended.clone();
    changed.replace(1, derived("changed summary"));
    assert!(
        anchor
            .covered_prefix_model_input(&changed, "fixed", &tools)
            .is_none()
    );
    let reordered: SelectedModelInput = [flat[2].clone(), flat[1].clone(), flat[0].clone()]
        .into_iter()
        .collect();
    assert!(
        anchor
            .covered_prefix_model_input(&reordered, "fixed", &tools)
            .is_none()
    );
    assert!(
        anchor
            .covered_prefix_model_input(&extended, "other", &tools)
            .is_none()
    );
    let mut changed_tools = tools.clone();
    changed_tools[0].input_schema["description"] = "different catalog".into();
    assert!(
        anchor
            .covered_prefix_model_input(&extended, "fixed", &changed_tools)
            .is_none()
    );
    assert!(anchor.matches_selected(SurfaceRevision::new(3), &selected, "fixed", &tools));
    let compacted: SelectedModelInput = vec![derived("new projection"), flat[2].clone()].into();
    assert!(
        anchor
            .covered_prefix_model_input(&compacted, "fixed", &tools)
            .is_none()
    );
}

#[test]
fn shared_measurement_at_100_300_1000_never_reserializes_the_old_prefix() {
    let engine = engine(Arc::new(DefaultTokenEstimator));
    let mut input = SelectedModelInput::default();
    for index in 0..=1000 {
        input.push(user(index, "immutable history 水 \\\""));
        if ![100, 300, 1000].contains(&input.len()) {
            continue;
        }
        let revision = SurfaceRevision::new(input.len() as u64);
        let initial = engine.build_projection_input(revision, input.clone(), &[], None, "");
        let observed = ProviderObservedInput {
            input_tokens: initial.estimated_input.input_tokens,
            anchor: ObservedAnchor::of_selected(revision, &input, "", &[]),
        };
        take_accounted_bytes();
        crate::model::selected_input::take_prefix_work();
        let exact =
            engine.build_projection_input(revision, input.clone(), &[], Some(&observed), "");
        assert_eq!(
            exact.estimated_input.source,
            TokenMeasurementSource::ProviderReported
        );
        assert_eq!(
            take_accounted_bytes(),
            0,
            "the exact root needs no token serialization"
        );
        assert_eq!(crate::model::selected_input::take_prefix_work(), (1, 0));
        let next = user(index + 1, "new tail 水 \\\n");
        let mut extended = input.clone();
        extended.push(next.clone());
        let height = usize::from(extended.0.root.as_ref().unwrap().height);
        let anchored =
            engine.build_projection_input(revision.next(), extended, &[], Some(&observed), "");
        let new_bytes = take_accounted_bytes();
        let (nodes, leaves) = crate::model::selected_input::take_prefix_work();
        assert!(
            nodes <= 8 * height + 8,
            "prefix comparison follows changed AVL paths"
        );
        assert_eq!(leaves, 0, "shared old leaves require no content comparison");
        assert!(new_bytes <= model_visible_item_bytes(&next).unwrap() + 2);
        assert_eq!(
            anchored.estimated_input.source,
            TokenMeasurementSource::ProviderAnchored
        );
        assert_eq!(
            anchored.estimated_input.input_tokens,
            observed.input_tokens + DefaultTokenEstimator.estimate_input(&[next], "", &[])
        );
        assert_eq!(input.len(), usize::try_from(revision.get()).unwrap());
    }
}

#[test]
fn arbitrary_nonadditive_estimator_receives_the_complete_original_selected_input() {
    let estimator = ClosureTokenEstimator::new(|messages, prompt, tools| {
        (messages.len() as u64).pow(2) * 11
            + DefaultTokenEstimator::serialized_bytes(messages, prompt, tools) % 97
    });
    let flat = vec![user(1, "a"), derived("b"), user(2, "c")];
    let selected: SelectedModelInput = flat.clone().into();
    let tools = tools();
    assert_eq!(
        estimator.estimate_selected(&selected, "fixed", &tools),
        estimator.estimate_input(&flat, "fixed", &tools)
    );
    let expected = estimator.estimate_input(&flat, "fixed", &tools);
    let projection = engine(Arc::new(estimator)).build_projection_input(
        SurfaceRevision::new(3),
        selected,
        &tools,
        None,
        "fixed",
    );
    assert_eq!(projection.estimated_input.input_tokens, expected);
    assert_eq!(
        projection.estimated_input.source,
        TokenMeasurementSource::Estimated
    );
}
