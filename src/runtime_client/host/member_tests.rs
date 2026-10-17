// Actual Native C/J receipts and finite Host checkpoints share one member owner.
use super::*;
use crate::runtime_client::checkpoint::RuntimeClientCheckpoint;
use crate::runtime_client::content::CanonicalContentLocator;
use crate::runtime_client::snapshot::RuntimeClientMessageDescriptor;

fn member_event(
    store: &dyn ConversationStore,
    attempt: &AttemptId,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
        event_id: crate::runtime::identity::EventId::new(format!(
            "host-member-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: Some(attempt.clone()),
        turn_id: None,
        timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        event,
    }
}

fn install_member_commit(inner: &Arc<super::super::ClientInner>, event: &RuntimeEventEnvelope) {
    let batch = event
        .observation
        .as_ref()
        .expect("same-commit native receipt");
    let sink = super::super::NativePublicationProjection {
        inner: Arc::downgrade(inner),
    };
    assert!(
        crate::runtime::observation::PublicationProjectionCommit::native_batch(&sink, batch)
            .is_installed()
    );
    if let Some(canonical) = &batch.canonical {
        inner
            .store
            .mark_canonical_transcript_ready(canonical.first_position, canonical.last_position)
            .unwrap();
    }
    if let Some((first, last)) = batch.journal_range {
        inner.store.mark_observation_direct(first, last).unwrap();
    }
}

fn member_messages() -> [MessageBlock; 2] {
    // The next native append must reopen this model head (ID <=4096 bytes),
    // while the first identity still exceeds the finite display inline bound.
    let intermediate = "host-intermediate🙂水".repeat(125);
    assert!(intermediate.len() > 256 && intermediate.len() <= 4096);
    // The closing C is last: Source observation and lineage never admit this
    // >1MiB original identity to the native model working set afterward.
    let closing = "host-closing🙂水".repeat(80_000);
    assert!(closing.len() > 1_048_576);
    [
        MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: MessageId::new(intermediate),
            content: vec![AssistantContentBlock::ToolCall(crate::tools::ToolCall {
                id: ToolCallId::new("host-member-call"),
                tool_id: ToolId::new("host-member-tool"),
                name: "native-proposal".into(),
                arguments: serde_json::json!({"ordinal":u64::MAX,"text":"水"}),
            })],
        }),
        MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: MessageId::new(closing),
            content: vec![AssistantContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "original closing body".into(),
                },
            )],
        }),
    ]
}

fn member_descriptor(
    checkpoint: &RuntimeClientCheckpoint,
    position: u64,
) -> RuntimeClientMessageDescriptor {
    checkpoint
        .snapshot
        .transcript
        .deferred_entries
        .iter()
        .find_map(|row| {
            let descriptor = crate::runtime_client::structural_json::decode::<
                RuntimeClientMessageDescriptor,
            >(row.value.clone()?)
            .ok()?;
            (descriptor.locator == CanonicalContentLocator::ByLedgerPosition { position })
                .then_some(descriptor)
        })
        .expect("actual deferred native descriptor")
}

fn assert_member_frontiers(
    before: &RuntimeClientMessageDescriptor,
    after: &RuntimeClientMessageDescriptor,
    before_journal: u64,
    after_journal: u64,
) {
    assert_eq!(before.collections.len(), after.collections.len());
    let mut tools = 0;
    for (before, after) in before.collections.iter().zip(&after.collections) {
        assert_eq!(before.domain, after.domain);
        if before.domain == "messages.ledger:1.tools" {
            tools += 1;
            assert_eq!(before.revision, before_journal);
            assert_eq!(after.revision, after_journal);
            assert_eq!(before.total, after.total);
            assert_eq!(before.loaded, after.loaded);
            assert_eq!(before.next_offset, after.next_offset);
        } else {
            assert_eq!(before, after);
        }
    }
    assert_eq!(tools, 1, "the native proposal retains its Tool frontier");
}

fn assert_completed_member_head(
    checkpoint: &RuntimeClientCheckpoint,
    origin: &ConversationId,
    attempt: &AttemptId,
) {
    let first = member_descriptor(checkpoint, 1);
    let final_row = member_descriptor(checkpoint, 2);
    let owner = first.turn_process.as_ref().unwrap();
    assert_eq!(owner, final_row.turn_process.as_ref().unwrap());
    assert_eq!(&owner.conversation_id, origin);
    assert_eq!(owner.attempt_id.as_ref(), Some(attempt));
    assert!(owner.attempt_id_source.is_none());
    assert_eq!(
        owner.outcome,
        crate::runtime_client::response::TurnProcessOutcome::Completed
    );
    assert_eq!(owner.control_cursor.get(), 1);
    assert_eq!(owner.message_count, 1);
    assert_eq!(owner.tool_call_count, 1);
    assert!(owner.final_message_id.is_none());
    assert!(owner.final_message_id_deferred);
    assert_eq!(owner.final_message_locator, Some(final_row.locator.clone()));
    assert!(owner.final_message_id_source.is_some());
    assert!(first.completed_response.is_none());
    let response = final_row.completed_response.as_ref().unwrap();
    assert_eq!(response.closing_locator, final_row.locator);
    assert_eq!(&response.origin.conversation_id, origin);
    assert_eq!(response.origin.attempt_id.as_ref(), Some(attempt));
    assert!(response.origin.attempt_id_source.is_none());
    assert!(response.closing_message_id.is_none());
    assert_eq!(first.tool_call_count, 1);
    assert_eq!(first.tool_calls.len(), 1);
    let call = &first.tool_calls[0];
    assert_eq!(
        call.call_id.as_ref(),
        Some(&ToolCallId::new("host-member-call"))
    );
    assert!(matches!(
        call.state,
        crate::runtime_client::snapshot::ForegroundToolState::Assembled { .. }
    ));
    assert!(crate::runtime_client::checkpoint::bounded_bytes(checkpoint, 786_432).is_some());
}

fn copied_member_host(
    store: &dyn ConversationStore,
) -> (
    tempfile::TempDir,
    RuntimeClientHost,
    Arc<crate::durable::SqliteConversationStore>,
) {
    let source_cut = store.observation_read_cut().unwrap();
    let cut = store
        .read_lineage_cut(source_cut.read.surface_revision)
        .unwrap();
    let seed = crate::durable::inbox::LineageSeed::replayed(cut.canonical, cut.surface_history)
        .unwrap()
        .with_projections(cut.projections)
        .unwrap()
        .with_turns(cut.turns)
        .unwrap()
        .with_completed_responses(cut.completed_responses)
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let id = ConversationId::generate();
    let path = directory.path().join("inherited.sqlite");
    {
        let destination = crate::durable::SqliteConversationStore::open(id.clone(), &path).unwrap();
        destination.initialize_lineage(&seed).unwrap();
        assert!(destination.read_events(None, 32).unwrap().events.is_empty());
    }
    let destination = Arc::new(crate::durable::SqliteConversationStore::open(id, &path).unwrap());
    let host = RuntimeClientHost::new_durable(destination.clone(), None).unwrap();
    (directory, host, destination)
}

struct NativeMemberFixture {
    adapter: Arc<GatedAdapter>,
    fixture: HostFixture,
    inner: Arc<super::super::ClientInner>,
    store: Arc<dyn ConversationStore>,
    attempt: AttemptId,
    running: RuntimeClientCheckpoint,
}

async fn native_member_fixture() -> NativeMemberFixture {
    let (adapter, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
    let inner = fixture.host.weak_inner().upgrade().unwrap();
    let store = fixture.runtime.tool_runtime().durable_store();
    let attempt = AttemptId::new("host-native-member-owner");
    inner.park_projection_worker();
    let start = store
        .append_event(member_event(
            store.as_ref(),
            &attempt,
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    install_member_commit(&inner, &start);
    for message in member_messages() {
        let (event, _) = store
            .append_canonical_with_event(
                &message,
                member_event(
                    store.as_ref(),
                    &attempt,
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: message.id().clone(),
                    },
                ),
            )
            .unwrap();
        install_member_commit(&inner, &event);
    }
    inner.unpark_projection_worker();
    await_pending_fixture_publication(&fixture.host).await;
    let running = fixture.host.checkpoint().unwrap();
    let first = member_descriptor(&running, 1);
    let last = member_descriptor(&running, 2);
    for row in [&first, &last] {
        assert!(row.identity.is_none());
        assert_eq!(
            row.turn_process.as_ref().unwrap().outcome,
            crate::runtime_client::response::TurnProcessOutcome::Running
        );
        assert!(
            row.turn_process
                .as_ref()
                .unwrap()
                .final_message_locator
                .is_none()
        );
        assert!(row.completed_response.is_none());
        assert!(!row.content.is_empty());
    }

    NativeMemberFixture {
        adapter,
        fixture,
        inner,
        store,
        attempt,
        running,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t12_t14_t19_host_giant_members_keep_exact_native_and_inherited_process() {
    let case = native_member_fixture().await;
    let first = member_descriptor(&case.running, 1);
    let last = member_descriptor(&case.running, 2);
    let original_window = case
        .inner
        .checkpoint_page(&case.running.read_cut, "transcript.entries", 0, 32)
        .unwrap();
    case.inner.park_projection_worker();
    let terminal = case
        .store
        .append_event(member_event(
            case.store.as_ref(),
            &case.attempt,
            RuntimeEvent::AttemptCompleted {
                attempt_id: case.attempt.clone(),
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
    install_member_commit(&case.inner, &terminal);
    case.inner.unpark_projection_worker();
    await_pending_fixture_publication(&case.fixture.host).await;
    let completed = case.fixture.host.checkpoint().unwrap();
    assert!(completed.cursor > case.running.cursor);
    assert_eq!(completed.read_cut.through, terminal.sequence);
    assert_completed_member_head(&completed, case.store.conversation_id(), &case.attempt);
    assert_eq!(member_descriptor(&completed, 1).content, first.content);
    assert_member_frontiers(
        &first,
        &member_descriptor(&completed, 1),
        case.running.read_cut.through,
        completed.read_cut.through,
    );
    assert_eq!(
        member_descriptor(&completed, 1).tool_calls,
        first.tool_calls
    );
    assert_eq!(member_descriptor(&completed, 2).content, last.content);
    assert_eq!(
        case.inner
            .checkpoint_page(&case.running.read_cut, "transcript.entries", 0, 32)
            .unwrap(),
        original_window
    );
    let original = case
        .store
        .read_events(Some(terminal.sequence - 1), 1)
        .unwrap();
    assert_eq!(original.events[0].event, terminal.event);
    assert!(case.adapter.requests().is_empty());
    let (_directory, inherited, destination) = copied_member_host(case.store.as_ref());
    assert_ne!(destination.conversation_id(), case.store.conversation_id());
    let copied = inherited.checkpoint().unwrap();
    assert_completed_member_head(&copied, case.store.conversation_id(), &case.attempt);
    assert_eq!(copied.read_cut.through, 0);
    assert!(destination.read_events(None, 32).unwrap().events.is_empty());
    assert_eq!(inherited.checkpoint().unwrap(), copied);
    case.fixture.runtime.shutdown().await.unwrap();
}
