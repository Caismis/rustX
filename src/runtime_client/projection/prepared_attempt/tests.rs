use super::*;
use crate::message::types::ContentBlockIndex;
use crate::model::{ModelProtocol, catalog::ModelCapabilities, invocation::ModelInvocationView};
use crate::runtime::identity::{MessageId, RuntimeResourceRevision};
use crate::runtime_client::{
    checkpoint::{self, Capture},
    snapshot::{InFlightAssistantMessage, RuntimeClientAttemptPhase},
};

fn source() -> Arc<Option<RuntimeClientAttempt>> {
    let model = ModelInvocationView {
        model: crate::model::catalog::ModelRef::parse("test/model").unwrap(),
        protocol: ModelProtocol::OpenAiChatCompletions,
        context_window: 4096,
        model_max_output_tokens: 1024,
        max_output_tokens: 1024,
        profile: None,
        reasoning_enabled: false,
        request_params: serde_json::json!({"zero": -0.0, "nested": {"a": 1, "b": 2}})
            .as_object()
            .unwrap()
            .clone(),
        capabilities: ModelCapabilities::text_only(true, false),
        declared_capabilities: ModelCapabilities::text_only(true, false),
    };
    Arc::new(Some(RuntimeClientAttempt {
        attempt_id: None,
        attempt_id_source: None,
        phase: RuntimeClientAttemptPhase::Running,
        turn: 1,
        last_usage: None,
        in_flight: Some(InFlightAssistantMessage {
            message_id: MessageId::new("message"),
            blocks: (0..35)
                .map(|i| InFlightBlock::Text {
                    block_index: ContentBlockIndex::new(i),
                    text: format!("block {i}: 水\\\n"),
                })
                .collect(),
        }),
        foreground: Vec::new(),
        model: Some(Box::new(AttemptModelView {
            primary: model,
            summary: SummaryModelView::Session,
        })),
        execution_settings: Some(AdmittedSettings {
            read_image_active: Some(true),
            resource_revision: RuntimeResourceRevision::new(1),
            approval_mode: crate::runtime::types::ApprovalMode::default(),
        }),
    }))
}

fn captured(
    source: &RuntimeClientAttempt,
    items: Option<&Items>,
    pressure: usize,
) -> (Vec<u8>, Capture) {
    let mut capture = Capture::new(17);
    for _ in 0..pressure {
        capture.reserve(&"x".repeat(4000));
    }
    let blocks = source
        .in_flight
        .as_ref()
        .unwrap()
        .blocks
        .iter()
        .take(CHECKPOINT_PAGE_ITEMS)
        .enumerate()
        .map(|(i, value)| {
            if let Some(items) = items {
                live_block(&mut capture, &items.blocks[i], value)
            } else {
                capture.live_block(value)
            }
        })
        .collect::<Vec<_>>();
    let model = if let Some(items) = items {
        super::capture(&mut capture, &items.model, |c| {
            c.item("attempt.model", &source.model).flatten()
        })
        .into_owned()
    } else {
        capture.item("attempt.model", &source.model).flatten()
    };
    let settings = if let Some(items) = items {
        super::capture(&mut capture, &items.settings, |c| {
            c.item("attempt.execution_settings", &source.execution_settings)
                .flatten()
        })
        .into_owned()
    } else {
        capture
            .item("attempt.execution_settings", &source.execution_settings)
            .flatten()
    };
    (
        serde_json::to_vec(&(blocks, model, settings)).unwrap(),
        capture,
    )
}

#[test]
fn attempt_item_reuse_encodes_only_changed_block_and_preserves_old_owner() {
    let source = source();
    let cache = PreparedAttemptItems::default();
    let initial = cache.select(&source);
    assert_eq!(initial.blocks.len(), 32);
    let original = captured(source.as_ref().as_ref().unwrap(), Some(&initial), 0).0;
    checkpoint::take_item_work();
    assert_eq!(
        captured(
            source.as_ref().as_ref().unwrap(),
            Some(&cache.select(&source)),
            0
        )
        .0,
        original
    );
    assert_eq!(checkpoint::take_item_work(), [0, 0, 0]);
    let mut changed = Arc::clone(&source);
    let current = Arc::make_mut(&mut changed).as_mut().unwrap();
    current.turn += 1;
    let InFlightBlock::Text { text, .. } = &mut current.in_flight.as_mut().unwrap().blocks[3]
    else {
        unreachable!()
    };
    text.push_str(" next chunk");
    let next = cache.select(&changed);
    for i in 0..32 {
        assert_eq!(Arc::ptr_eq(&initial.blocks[i], &next.blocks[i]), i != 3);
    }
    assert!(Arc::ptr_eq(&initial.model, &next.model));
    assert!(Arc::ptr_eq(&initial.settings, &next.settings));
    checkpoint::take_item_work();
    let result = captured(changed.as_ref().as_ref().unwrap(), Some(&next), 0).0;
    assert_eq!(checkpoint::take_item_work(), [1, 1, 0]);
    assert_ne!(result, original);
    assert_eq!(
        captured(source.as_ref().as_ref().unwrap(), Some(&initial), 0).0,
        original
    );
    assert_eq!(
        Arc::strong_count(&source),
        1,
        "latest cache does not retain a prior source chain"
    );
}

#[test]
fn attempt_prepared_items_match_uncached_admission_at_shared_budget_boundaries() {
    let mut source = source();
    let attempt = Arc::make_mut(&mut source).as_mut().unwrap();
    let blocks = &mut attempt.in_flight.as_mut().unwrap().blocks;
    blocks[0] = InFlightBlock::Refusal {
        block_index: ContentBlockIndex::new(0),
        text: "x".repeat(checkpoint::CHECKPOINT_ITEM_BYTES),
    };
    blocks[1] = InFlightBlock::Reasoning {
        block_index: ContentBlockIndex::new(1),
        text: "水\"\n".repeat(900),
    };
    let items = PreparedAttemptItems::default().select(&source);
    let attempt = source.as_ref().as_ref().unwrap();
    for pressure in 0..=40 {
        let (expected, ec) = captured(attempt, None, pressure);
        let (actual, ac) = captured(attempt, Some(&items), pressure);
        assert_eq!(actual, expected, "pressure {pressure}");
        assert_eq!(
            ac.admission_state(),
            ec.admission_state(),
            "pressure {pressure}"
        );
        assert_eq!(ac.collections, ec.collections);
        assert_eq!(
            serde_json::to_vec(&ac.content).unwrap(),
            serde_json::to_vec(&ec.content).unwrap()
        );
    }
}

#[test]
fn attempt_preparation_invalidates_order_signed_zero_settings_and_position() {
    let source = source();
    let cache = PreparedAttemptItems::default();
    let first = cache.select(&source);
    let mut changed = Arc::clone(&source);
    Arc::make_mut(&mut changed)
        .as_mut()
        .unwrap()
        .model
        .as_mut()
        .unwrap()
        .primary
        .request_params
        .insert("zero".into(), serde_json::json!(0.0));
    assert!(!Arc::ptr_eq(&first.model, &cache.select(&changed).model));
    let first = cache.select(&source);
    let mut changed = Arc::clone(&source);
    let params = &mut Arc::make_mut(&mut changed)
        .as_mut()
        .unwrap()
        .model
        .as_mut()
        .unwrap()
        .primary
        .request_params;
    let value = params.shift_remove("zero").unwrap();
    params.insert("zero".into(), value);
    assert!(!Arc::ptr_eq(&first.model, &cache.select(&changed).model));
    let first = cache.select(&source);
    let mut changed = Arc::clone(&source);
    let attempt = Arc::make_mut(&mut changed).as_mut().unwrap();
    attempt
        .execution_settings
        .as_mut()
        .unwrap()
        .resource_revision = RuntimeResourceRevision::new(2);
    attempt.in_flight.as_mut().unwrap().blocks.swap(1, 2);
    let next = cache.select(&changed);
    assert!(!Arc::ptr_eq(&first.settings, &next.settings));
    assert!(!Arc::ptr_eq(&first.blocks[1], &next.blocks[1]));
    assert!(!Arc::ptr_eq(&first.blocks[2], &next.blocks[2]));
    let mut none = Arc::clone(&changed);
    *Arc::make_mut(&mut none) = None;
    let empty = cache.select(&none);
    assert!(empty.blocks.is_empty() && empty.tools.is_empty());
}

fn tool() -> ForegroundToolExecution {
    use crate::runtime_client::{
        content::CanonicalContentLocator, snapshot::RuntimeClientToolOccurrence,
    };
    ForegroundToolExecution {
        occurrence: RuntimeClientToolOccurrence {
            locator: CanonicalContentLocator::ByLedgerPosition { position: 1 },
            block_index: ContentBlockIndex::new(0),
        },
        message_id: None,
        block_index: ContentBlockIndex::new(0),
        call_id: None,
        metadata_ref: None,
        assembly_complete: true,
        result_source: None,
        result_metadata_ref: None,
        committed_result: None,
        committed_result_deferred: false,
        tool_id: None,
        name: "tool".into(),
        state: ForegroundToolState::Running {
            arguments: " {\"raw\": 1e+003} ".into(),
            progress: Some(crate::tools::types::ToolProgress {
                message: None,
                completed: Some(-0.0),
                total: Some(10.0),
            }),
        },
    }
}

#[test]
fn tool_preparation_replays_reserve_without_reusing_decorated_sources() {
    let mut source = source();
    Arc::make_mut(&mut source)
        .as_mut()
        .unwrap()
        .foreground
        .push(tool());
    let cache = PreparedAttemptItems::default();
    let first = cache.select(&source);
    let original = &source.as_ref().as_ref().unwrap().foreground[0];
    for pressure in 0..=40 {
        let mut expected = Capture::new(9);
        let mut actual = Capture::new(9);
        for _ in 0..pressure {
            expected.reserve(&"x".repeat(4000));
            actual.reserve(&"x".repeat(4000));
        }
        assert_eq!(
            *super::capture(&mut actual, &first.tools[0], |c| c.reserve(original)),
            expected.reserve(original)
        );
        assert_eq!(actual.admission_state(), expected.admission_state());
    }
    let mut changed = Arc::clone(&source);
    if let ForegroundToolState::Running {
        progress: Some(progress),
        ..
    } = &mut Arc::make_mut(&mut changed).as_mut().unwrap().foreground[0].state
    {
        progress.completed = Some(0.0);
    }
    assert!(!Arc::ptr_eq(
        &first.tools[0],
        &cache.select(&changed).tools[0]
    ));
    assert!(
        !Arc::ptr_eq(&first.tools[0], &cache.select(&source).tools[0]),
        "returning to an older input uses a new latest proof, not history"
    );
}

fn live_text(block: &InFlightBlock) -> &String {
    match block {
        InFlightBlock::Text { text, .. }
        | InFlightBlock::Reasoning { text, .. }
        | InFlightBlock::Refusal { text, .. } => text,
        InFlightBlock::ToolCall { arguments, .. } => arguments,
    }
}

#[test]
fn growing_stream_moves_each_normalized_allocation_without_fit_or_extra_copy() {
    let mut current = source();
    Arc::make_mut(&mut current)
        .as_mut()
        .unwrap()
        .in_flight
        .as_mut()
        .unwrap()
        .blocks
        .truncate(1);
    let cache = PreparedAttemptItems::default();
    let original = Arc::clone(&current);
    let original_items = cache.select(&original);
    checkpoint::take_prepared_fit_work();
    let original_bytes = captured(
        original.as_ref().as_ref().unwrap(),
        Some(&original_items),
        0,
    )
    .0;
    assert_eq!(checkpoint::take_prepared_fit_work(), 0);
    for _ in 0..128 {
        let attempt = Arc::make_mut(&mut current).as_mut().unwrap();
        let InFlightBlock::Text { text, .. } = &mut attempt.in_flight.as_mut().unwrap().blocks[0]
        else {
            unreachable!()
        };
        text.push_str("abcdefghijklmnopqr");
        let items = cache.select(&current);
        assert!(Arc::ptr_eq(&original_items.model, &items.model));
        assert!(Arc::ptr_eq(&original_items.settings, &items.settings));
        let attempt = current.as_ref().as_ref().unwrap();
        let block = &attempt.in_flight.as_ref().unwrap().blocks[0];
        let expected = Capture::new(17).live_block(block).unwrap();
        checkpoint::take_item_work();
        checkpoint::take_prepared_fit_work();
        NORMALIZED_ALLOCATION.with(|allocation| allocation.set(None));
        let actual = live_block(&mut Capture::new(17), &items.blocks[0], block).unwrap();
        assert_eq!(checkpoint::take_item_work(), [1, 1, 0]);
        assert_eq!(checkpoint::take_prepared_fit_work(), 0);
        let allocation = NORMALIZED_ALLOCATION
            .with(|allocation| allocation.replace(None))
            .unwrap();
        assert_eq!(
            allocation,
            (
                live_text(&actual).as_ptr() as usize,
                live_text(&actual).capacity()
            ),
            "cold output must retain the buffer produced by normalization"
        );
        assert_eq!(
            live_text(&actual).capacity(),
            live_text(&expected).capacity()
        );
        assert_eq!(actual, expected);
        let warm = live_block(&mut Capture::new(17), &items.blocks[0], block).unwrap();
        assert_eq!(warm, expected);
        assert_eq!(checkpoint::take_item_work(), [0, 0, 0]);
        assert_eq!(checkpoint::take_prepared_fit_work(), 0);
        // Model and settings also use admission-only preparations, including
        // their cold initial capture above and replay at the current revision.
        checkpoint::take_prepared_fit_work();
        assert_eq!(
            captured(attempt, Some(&items), 0).0,
            captured(attempt, None, 0).0
        );
        assert_eq!(checkpoint::take_prepared_fit_work(), 0);
    }
    assert_eq!(
        captured(
            original.as_ref().as_ref().unwrap(),
            Some(&original_items),
            0
        )
        .0,
        original_bytes
    );
}

#[test]
fn live_admission_cache_matches_cold_and_warm_original_at_exact_budget_boundaries() {
    let tool = tool();
    for text in [
        "水\"\n\\ 1e+003 ".repeat(30),
        "x".repeat(checkpoint::CHECKPOINT_ITEM_BYTES),
    ] {
        for block in [
            InFlightBlock::Text {
                block_index: ContentBlockIndex::new(0),
                text: text.clone(),
            },
            InFlightBlock::Reasoning {
                block_index: ContentBlockIndex::new(1),
                text: text.clone(),
            },
            InFlightBlock::Refusal {
                block_index: ContentBlockIndex::new(2),
                text: text.clone(),
            },
            InFlightBlock::ToolCall {
                block_index: ContentBlockIndex::new(3),
                call_id: tool.call_id.clone(),
                tool_id: tool.tool_id.clone(),
                metadata_ref: tool.metadata_ref.clone(),
                name: tool.name.clone(),
                arguments: text.clone(),
            },
        ] {
            let encoded = serde_json::to_vec(&block).unwrap().len();
            let required = crate::runtime_client::preflight::required_budget(
                &block,
                checkpoint::CHECKPOINT_TARGET_BYTES,
            )
            .unwrap();
            for remaining in [
                0,
                1,
                encoded - 1,
                encoded,
                required - 1,
                required,
                required + 1,
                checkpoint::CHECKPOINT_ITEM_BYTES,
            ] {
                let owner = OnceLock::new();
                let mut expected_capture = Capture::with_remaining(7, remaining);
                checkpoint::take_item_work();
                let expected = expected_capture.live_block(&block);
                let work = checkpoint::take_item_work();
                let mut cold_capture = Capture::with_remaining(7, remaining);
                let cold = live_block(&mut cold_capture, &owner, &block);
                assert_eq!(
                    checkpoint::take_item_work(),
                    work,
                    "cold budget rejection must not encode twice"
                );
                assert_eq!(cold, expected);
                assert_eq!(
                    cold_capture.admission_state(),
                    expected_capture.admission_state()
                );
                let mut warm_capture = Capture::with_remaining(19, remaining);
                assert_eq!(live_block(&mut warm_capture, &owner, &block), expected);
                assert_eq!(checkpoint::take_item_work(), [0, 0, 0]);
                assert_eq!(
                    warm_capture.admission_state(),
                    expected_capture.admission_state()
                );
                assert_eq!(warm_capture.collections, expected_capture.collections);
                assert_eq!(
                    serde_json::to_vec(&warm_capture.content).unwrap(),
                    serde_json::to_vec(&expected_capture.content).unwrap()
                );
            }
        }
    }
}

#[test]
fn nonidentical_normalized_body_is_never_replaced_by_the_original_on_replay() {
    let source = InFlightBlock::Text {
        block_index: ContentBlockIndex::new(0),
        text: "source".into(),
    };
    let normalized = InFlightBlock::Text {
        block_index: ContentBlockIndex::new(0),
        text: "normalized value".into(),
    };
    let owner = OnceLock::new();
    // Exercise the defensive branch with an actual prepared value that differs
    // from its source. Every replay must keep using that normalization operation.
    for remaining in [
        checkpoint::CHECKPOINT_ITEM_BYTES,
        0,
        checkpoint::CHECKPOINT_ITEM_BYTES,
    ] {
        let mut actual_capture = Capture::with_remaining(1, remaining);
        let actual = live_block_with(&mut actual_capture, &owner, &source, || {
            prepare_capture_item(&normalized)
        });
        let mut expected_capture = Capture::with_remaining(1, remaining);
        assert_eq!(actual, expected_capture.live_block(&normalized));
        assert_eq!(
            actual_capture.admission_state(),
            expected_capture.admission_state()
        );
        assert!(matches!(owner.get(), Some(PreparedLiveBlock::Normalize)));
    }
}
