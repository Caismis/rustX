use crate::runtime_client::projection::block_index_of;
#[test]
fn attempt_domains_detach_only_the_updated_block_and_keep_issued_cuts() {
    let mut projection = projection();
    projection.ensure_attempt(&attempt());
    let tool = foreground(
        ForegroundToolState::Assembled {
            arguments: "{\"x\":1}".into(),
        },
        false,
    );
    let state = projection.snapshot.attempt.as_mut().unwrap();
    state.foreground = vec![tool].into();
    state.in_flight = Some(InFlightAssistantMessage {
        message_id: MessageId::new("stream"),
        blocks: (0..3)
            .map(|index| InFlightBlock::Text {
                block_index: ContentBlockIndex::new(index),
                text: "水".repeat(1000),
            })
            .collect(),
    })
    .into();
    let original = projection.capture_working_seed();
    let old = original.snapshot.attempt.as_ref().unwrap();
    let bytes = serde_json::to_vec(&old.owned()).unwrap();
    let old_flight = old.in_flight.as_ref().unwrap();
    let mut next = original.clone();
    let current = next.snapshot.attempt.as_mut().unwrap();
    let changed = current
        .in_flight
        .as_mut()
        .unwrap()
        .blocks
        .find_mut(|block| block_index_of(block) == ContentBlockIndex::new(2))
        .unwrap();
    let InFlightBlock::Text { text, .. } = changed else {
        unreachable!()
    };
    text.push_str(" next");
    assert!(current.model.same_owner(&old.model));
    assert!(
        current
            .execution_settings
            .same_owner(&old.execution_settings)
    );
    assert!(current.foreground.same_owner(&old.foreground));
    let flight = current.in_flight.as_ref().unwrap();
    for index in 0..3 {
        assert_eq!(
            flight.blocks.same_block(&old_flight.blocks, index),
            index != 2
        );
    }
    assert_eq!(serde_json::to_vec(&old.owned()).unwrap(), bytes);
    assert_ne!(serde_json::to_vec(&current.owned()).unwrap(), bytes);
}

#[test]
fn attempt_view_borrows_warm_blocks_and_tools_without_changing_public_wire() {
    let mut projection = projection();
    projection.ensure_attempt(&attempt());
    let state = projection.snapshot.attempt.as_mut().unwrap();
    state.foreground = vec![foreground(
        ForegroundToolState::Running {
            arguments: "{ \"exact\":1e+003 }".into(),
            progress: None,
        },
        false,
    )]
    .into();
    state.in_flight = Some(InFlightAssistantMessage {
        message_id: MessageId::new("stream"),
        blocks: vec![InFlightBlock::Reasoning {
            block_index: ContentBlockIndex::new(0),
            text: "水\"\\\n".repeat(30),
        }],
    })
    .into();
    let seed = projection.capture_working_seed();
    let mut warm = Capture::new(17);
    let first = seed.capture_attempt_view(&mut warm).unwrap();
    super::super::prepared_attempt::take_budget_work();
    let mut capture = Capture::new(17);
    let second = seed.capture_attempt_view(&mut capture).unwrap();
    assert_eq!(super::super::prepared_attempt::take_budget_work(), 0);
    assert_eq!(
        second.fit,
        crate::observation::preflight::required_safe_integer_budget(
            &second,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES
        )
    );
    assert!(Arc::ptr_eq(&first.model, &second.model));
    assert!(Arc::ptr_eq(
        &first.execution_settings,
        &second.execution_settings
    ));
    assert!(matches!(
        second.in_flight.as_ref().unwrap().blocks[0],
        std::borrow::Cow::Borrowed(_)
    ));
    assert!(matches!(
        second.foreground[0],
        std::borrow::Cow::Borrowed(_)
    ));
    let public = second.into_owned();
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&public).unwrap()
    );
    assert_eq!(warm.admission_state(), capture.admission_state());
    assert_eq!(warm.collections, capture.collections);
    assert_eq!(
        serde_json::to_vec(&warm.content).unwrap(),
        serde_json::to_vec(&capture.content).unwrap()
    );
}

#[test]
fn borrowed_attempt_keeps_budget_rejection_frontiers_and_old_owner_after_mutation() {
    let mut projection = projection();
    projection.ensure_attempt(&attempt());
    projection.snapshot.attempt.as_mut().unwrap().in_flight = Some(InFlightAssistantMessage {
        message_id: MessageId::new("stream"),
        blocks: vec![InFlightBlock::Text {
            block_index: ContentBlockIndex::new(0),
            text: "水".repeat(3000),
        }],
    })
    .into();
    let seed = projection.capture_working_seed();
    let mut prior = Capture::new(17);
    let old = seed.capture_attempt(&mut prior).unwrap();
    let bytes = serde_json::to_vec(&old).unwrap();
    for remaining in [0, 1, 128, 8192, 16_384, 32_768] {
        let mut expected = Capture::with_remaining(17, remaining);
        let mut actual = Capture::with_remaining(17, remaining);
        let public = seed.capture_attempt(&mut expected).unwrap();
        let borrowed = seed.capture_attempt_view(&mut actual).unwrap();
        assert_eq!(
            borrowed.fit,
            crate::observation::preflight::required_safe_integer_budget(
                &borrowed,
                crate::observation::bounded::CHECKPOINT_TARGET_BYTES
            )
        );
        assert_eq!(
            serde_json::to_vec(&borrowed).unwrap(),
            serde_json::to_vec(&public).unwrap()
        );
        assert_eq!(actual.admission_state(), expected.admission_state());
        assert_eq!(actual.collections, expected.collections);
    }
    let mut changed = seed.clone();
    let InFlightBlock::Text { text, .. } = &mut changed
        .snapshot
        .attempt
        .as_mut()
        .unwrap()
        .in_flight
        .as_mut()
        .unwrap()
        .blocks[0]
    else {
        unreachable!()
    };
    text.push_str(" changed");
    let mut after = Capture::new(17);
    assert_eq!(
        serde_json::to_vec(&seed.capture_attempt(&mut after).unwrap()).unwrap(),
        bytes
    );
}

#[test]
fn composed_attempt_budget_keeps_unsafe_quantities_and_current_tool_decoration() {
    let mut projection = projection();
    projection.ensure_attempt(&attempt());
    let mut row = foreground(
        ForegroundToolState::Running {
            arguments: "{}".into(),
            progress: Some(ToolProgress {
                message: None,
                completed: Some(-0.0),
                total: Some(9.5),
            }),
        },
        false,
    );
    row.metadata_ref.as_mut().unwrap().readable_end = u64::MAX;
    projection.snapshot.attempt.as_mut().unwrap().foreground = vec![row].into();
    let seed = projection.capture_working_seed();
    for remaining in [0, 1, 1024, 16_384, 32_768] {
        let mut capture = Capture::with_remaining(17, remaining);
        let view = seed.capture_attempt_view(&mut capture).unwrap();
        assert_eq!(
            view.fit,
            crate::observation::preflight::required_safe_integer_budget(
                &view,
                crate::observation::bounded::CHECKPOINT_TARGET_BYTES
            )
        );
        assert_eq!(
            serde_json::to_vec(&view).unwrap(),
            serde_json::to_vec(&view.into_owned()).unwrap()
        );
    }
}
