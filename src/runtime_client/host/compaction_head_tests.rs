// A committed compaction owns its head before asynchronous domain refinement.
use super::*;

struct ParkedProjection<'a>(&'a super::super::ClientInner);
impl Drop for ParkedProjection<'_> {
    fn drop(&mut self) {
        self.0.unpark_projection_worker();
    }
}

async fn completed_turn(fixture: &HostFixture, subscription: &EventSubscription) {
    fixture
        .runtime
        .submit_inbound(submit_content("original turn before compaction"))
        .unwrap();
    receive_until(subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
        loop {
            let released = fixture.runtime.settlement_signal().notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if !fixture.runtime.has_current_attempt() {
                break;
            }
            released.await;
        }
    })
    .await
    .expect("native physical attempt owner releases");
    fixture.host.projection_settled().await;
}

fn assert_compaction_head(
    context: &crate::runtime_client::snapshot::RuntimeClientContextView,
    outcome: &crate::runtime::conversation_runtime::ManualCompactionOutcome,
) {
    assert_eq!(context.compaction_count, outcome.generation);
    let compaction = context.latest_compaction.as_ref().unwrap();
    assert_eq!(compaction.generation, outcome.generation);
    assert_eq!(compaction.activation_id, outcome.activation_id);
    assert_eq!(compaction.checkpoint_id, outcome.checkpoint_id);
    let head = context.projection_head.as_ref().unwrap();
    assert_eq!(head.activation_id, outcome.activation_id);
    assert_eq!(head.checkpoint_id, outcome.checkpoint_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_compaction_head_is_atomic_with_context_while_domain_reads_are_parked() {
    let (model, fixture) = host_fixture(
        (0..4).map(|_| one_turn_stop()).collect(),
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let initial = fixture.host.checkpoint().unwrap();
    let subscription = attachment.subscribe_events(initial.cursor).unwrap();
    completed_turn(&fixture, &subscription).await;
    let first = fixture.runtime.compact_context(None).await.unwrap();
    fixture.host.projection_settled().await;
    assert_compaction_head(&fixture.host.checkpoint().unwrap().snapshot.context, &first);
    completed_turn(&fixture, &subscription).await;
    let before = fixture.host.checkpoint().unwrap();
    assert_compaction_head(&before.snapshot.context, &first);

    fixture.host.inner.park_projection_worker();
    let parked = ParkedProjection(&fixture.host.inner);
    let subscription = attachment.subscribe_events(before.cursor).unwrap();
    let second = fixture.runtime.compact_context(None).await.unwrap();
    let after = fixture.host.checkpoint().unwrap();
    assert_eq!(second.generation, 2);
    assert_compaction_head(&after.snapshot.context, &second);
    let store = fixture.store.as_ref().unwrap();
    let native = store.observation_read_cut().unwrap();
    assert_eq!(after.read_cut.source_cut.as_ref(), Some(&native));
    assert_eq!(after.read_cut.through, native.read.journal);
    assert_eq!(
        after.snapshot.context.projection_head,
        store
            .observation_projection_head(native.read.journal)
            .unwrap()
    );
    assert_compacted_replay(&subscription, before.cursor, &after, &second).await;
    let original = store.read_events(None, 128).unwrap().events;
    let completions: Vec<_> = original
        .iter()
        .filter(|event| matches!(event.event, RuntimeEvent::CompactionCompleted { .. }))
        .collect();
    assert_eq!(completions.len(), 2);
    assert_eq!(completions[1].sequence, native.read.journal);
    assert_eq!(model.requests().len(), 4);
    drop(parked);
    fixture.host.projection_settled().await;
    assert_compaction_head(
        &fixture.host.checkpoint().unwrap().snapshot.context,
        &second,
    );
    fixture.runtime.shutdown().await.unwrap();
    fixture.host.inner.drain_projection().await.unwrap();
}

async fn assert_compacted_replay(
    subscription: &EventSubscription,
    before: RuntimeClientCursor,
    after: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    outcome: &crate::runtime::conversation_runtime::ManualCompactionOutcome,
) {
    let mut cursor = before;
    let mut completed = 0;
    while cursor < after.cursor {
        let EventDelivery::Event(record) = subscription.next().await else {
            panic!("the original complete transition remains replayable");
        };
        assert_eq!(record.cursor.get(), cursor.get() + 1);
        cursor = record.cursor;
        let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &record.event else {
            panic!("one complete publication record");
        };
        assert_eq!(metadata.read_cut.revision, record.cursor.get());
        for fact in record.event.facts() {
            if let RuntimeClientEvent::ContextCompacted { context, .. } = fact {
                completed += 1;
                assert_compaction_head(context, outcome);
                assert_eq!(metadata.read_cut.through, after.read_cut.through);
                assert_eq!(metadata.read_cut.source_cut, after.read_cut.source_cut);
            }
        }
    }
    assert_eq!(cursor, after.cursor);
    assert_eq!(completed, 1);
}
