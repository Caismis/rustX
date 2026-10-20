/// Capacity suite: this test's value comes from its scale, so FAST skips
/// `::scale::` and FULL runs it (tests/README.md, "Capacity tests").
mod scale {
    use crate::durable::{CanonicalStore};
    use super::*;

    /// Exercise real preparation, native start, publication and provider dispatch after
    /// a warmed historical request. Only the explicit final oracle discloses history.
    #[tokio::test]
    async fn full_agent_plain_text_append_reuses_native_input_at_100_300_1000() {
        for count in [100usize, 300, 1000] {
            let adapter = Arc::new(ScriptedAdapter::new(vec![
                workflow_output_turn(&[], Some("warm original response")),
                workflow_output_turn(&[], Some("new original response")),
            ]));
            let store = Arc::new(
                crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                    "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                ))
                .unwrap(),
            );
            let tool_runtime = tool_runtime_with_store(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                Some(store.clone()),
            );
            let canonical: Vec<_> = (0..count)
                .map(|index| {
                    MessageBlock::User(inbound_message(
                        &format!("full-chain-{index}"),
                        &format!("original 水🙂 {index}"),
                    ))
                })
                .collect();
            let mut first = request(&adapter);
            first.conversation = ConversationState::from_messages(canonical.clone()).unwrap();
            let (_dir, _coordinator, lease) =
                capability_lease(ToolRegistry::new(), &tool_runtime).await;
            let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
            let warm = AgentExecution::new(
                first,
                lease,
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                runtime(&adapter),
                &tool_runtime,
                crate::agent::AttemptLifecycle::inert(),
            )
            .unwrap()
            .run()
            .await;
            assert!(matches!(warm.outcome, AttemptOutcome::Completed { .. }));
            let mut second = request(&adapter);
            second.attempt_id = AttemptId::new("full-chain-second");
            second.conversation = warm.conversation;
            let delta = MessageBlock::User(inbound_message("full-chain-new-user", "new exact input"));
            store.append_canonical(&delta).unwrap();
            second.conversation.commit(delta.clone()).unwrap();
            let (_dir, _coordinator, lease) =
                capability_lease(ToolRegistry::new(), &tool_runtime).await;
            crate::model::selected_input::take_preparation_visits();
            crate::model::selected_input::take_materialized_items();
            crate::model::selected_input::take_hash_work();
            crate::durable::sqlite::request::snapshot::take_request_input_reconstructions();
            let execution = AgentExecution::new(
                second,
                lease,
                &cancellation,
                crate::scripted_suites::support::default_execution_policy(),
                runtime(&adapter),
                &tool_runtime,
                crate::agent::AttemptLifecycle::inert(),
            )
            .unwrap();
            let result = execution.run().await;
            let visits = crate::model::selected_input::take_preparation_visits();
            let materialized = crate::model::selected_input::take_materialized_items();
            let hashes = crate::model::selected_input::take_hash_work();
            let reconstruction = crate::durable::sqlite::request::snapshot::take_request_input_reconstructions();
            assert!(
                matches!(result.outcome, AttemptOutcome::Completed { .. }),
                "{count}: {:?}",
                result.outcome
            );
            assert_eq!(adapter.request_count(), 2);
            assert!(
                visits < 64,
                "native preparation visits only actualdelta, not oldhistory at {count}: {visits}"
            );
            assert_eq!(
                materialized, 0,
                "shared native input is never flattened during preparation at {count}"
            );
            assert_eq!(
                reconstruction,
                (0, 0),
                "commit evidence replaces a second historical Store reconstruction"
            );
            assert!(
                hashes.0 < 16_384 && hashes.1 < 16_384,
                "only genuine new input is hashed at {count}: {hashes:?}"
            );
            assert_full_agent_input_oracle(store.as_ref(), &adapter, &canonical, &delta, count);
        }
    }
}

fn assert_full_agent_input_oracle(
    store: &crate::durable::SqliteConversationStore,
    adapter: &Arc<ScriptedAdapter>,
    canonical: &[MessageBlock],
    delta: &MessageBlock,
    count: usize,
) {
    let requests = adapter.requests();
    let actual = requests.last().unwrap();
    assert_eq!(
        actual.messages.range(0..count).to_vec(),
        crate::model::input::canonical_input(canonical)
    );
    assert!(
        actual
            .messages
            .iter()
            .any(|item| item.as_canonical() == Some(delta))
    );
    let snapshots = store.read_request_snapshots(None, 16).unwrap().snapshots;
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.identity.attempt_id == AttemptId::new("full-chain-second"))
        .unwrap();
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        *actual
    );
    let mut after = None;
    let mut terminals = 0;
    loop {
        let page = store.read_events(after, 32).unwrap();
        if page.events.is_empty() {
            break;
        }
        terminals += page
            .events
            .iter()
            .filter(|event| {
                event.attempt_id.as_ref() == Some(&AttemptId::new("full-chain-second"))
                    && AttemptOutcome::from_terminal_event(&event.event).is_some()
            })
            .count();
        assert!(page.next_sequence > after);
        after = page.next_sequence;
    }
    assert_eq!(terminals, 1);
}
