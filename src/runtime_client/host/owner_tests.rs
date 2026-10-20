// Actual cold checkpoints retain original inherited owner identity sources.
use crate::durable::{CanonicalStore, JournalStore, ObservationStore, ProjectionStore};
use super::*;
use crate::runtime_client::response::{AttemptIdentitySource, InheritedAttemptIdentityKind};

fn giant_owner_seed(
    with_turn: bool,
) -> (crate::durable::LineageSeed, ConversationId, String) {
    use crate::durable::reading::{
        ConversationTurnId, InheritedTurnOutcome, TurnExecution, TurnReadingProvenance,
    };
    use crate::durable::response::{CompletedResponseProvenance, ResponseOrigin};
    let original = "original-owner\"\n水🙂".repeat(80_000);
    assert!(original.len() > 1_048_576);
    let origin = ConversationId::generate();
    let attempt = AttemptId::new(&original);
    let ids = [MessageId::new("owner-first"), MessageId::new("owner-final")];
    let at = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let first = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
        id: ids[0].clone(),
        content: vec![AssistantContentBlock::ToolCall(crate::tools::ToolCall {
            id: ToolCallId::new("inherited-owner-call"),
            tool_id: ToolId::new("inherited-owner-tool"),
            name: "original-proposal".into(),
            arguments: serde_json::json!({"ordinal":u64::MAX}),
        })],
    });
    let final_message = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
        id: ids[1].clone(),
        content: vec![AssistantContentBlock::Text(
            crate::message::content::TextBlock {
                text: "original final body".into(),
            },
        )],
    });
    let mut seed = crate::durable::LineageSeed::history(vec![first, final_message]);
    if with_turn {
        seed = seed
            .with_turns(vec![TurnReadingProvenance {
                id: ConversationTurnId {
                    conversation_id: origin.clone(),
                    attempt_id: attempt.clone(),
                },
                process_message_ids: ids.to_vec(),
                preceding_message_id: None,
                prompt_message_id: None,
                outcome: InheritedTurnOutcome::Completed,
                started_at: Some(at),
                ended_at: Some(at),
                execution: Some(TurnExecution {
                    completed_response: true,
                    ..Default::default()
                }),
            }])
            .unwrap();
    }
    let seed = seed
        .with_completed_responses(vec![CompletedResponseProvenance {
            process_message_ids: ids.to_vec(),
            closing_message_id: ids[1].clone(),
            origin: ResponseOrigin {
                conversation_id: origin.clone(),
                attempt_id: attempt,
                closing_message_id: ids[1].clone(),
            },
            completed_at: at,
            retry_message_id: None,
            usage: None,
            timing: None,
            models: Vec::new(),
        }])
        .unwrap();
    (seed, origin, original)
}

fn read_original_owner(
    host: &RuntimeClientHost,
    checkpoint: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    source: &AttemptIdentitySource,
    expected_kind: InheritedAttemptIdentityKind,
    original: &str,
) {
    use base64::Engine as _;
    let AttemptIdentitySource::Inherited {
        kind,
        ordinal,
        domain,
        encoded_bytes,
    } = source
    else {
        panic!("actual inherited owner source");
    };
    assert_eq!(*kind, expected_kind);
    assert_eq!(*ordinal, 0);
    let expected = serde_json::to_vec(original).unwrap();
    assert_eq!(*encoded_bytes, expected.len() as u64);
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = host
            .inner
            .checkpoint_value_read(
                &checkpoint.read_cut,
                domain,
                0,
                &["attempt_id".into()],
                offset,
                65_536,
            )
            .unwrap();
        assert_eq!(chunk.cut, checkpoint.read_cut);
        assert_eq!(chunk.domain, *domain);
        assert_eq!(chunk.item_offset, 0);
        assert_eq!(chunk.path, ["attempt_id"]);
        assert_eq!(chunk.offset, offset);
        assert_eq!(chunk.readable_end, *encoded_bytes);
        let data = base64::engine::general_purpose::STANDARD
            .decode(chunk.bytes_base64)
            .unwrap();
        assert!(!data.is_empty() && data.len() <= 65_536);
        bytes.extend(data);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes, expected);
    assert_eq!(host.checkpoint().unwrap(), *checkpoint);
}

#[tokio::test]
async fn t08_t12_t14_t19_cold_host_giant_owner_uses_original_inherited_identity_ranges() {
    let (seed, origin, original) = giant_owner_seed(true);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("giant-owner.sqlite");
    let conversation = ConversationId::generate();
    {
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize_lineage(&seed).unwrap();
        assert_eq!(owner.load_inherited_turns().unwrap(), seed.turns());
        assert_eq!(owner.load_canonical().unwrap().as_slice(), seed.canonical());
    }
    let store =
        Arc::new(SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap());
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    assert_eq!(checkpoint.cursor.get(), 0);
    assert_eq!(checkpoint.read_cut.through, 0);
    assert_eq!(checkpoint.read_cut.conversation_id, conversation);
    assert!(
        checkpoint.snapshot.attempt.is_none(),
        "no local execution is invented"
    );
    assert_eq!(checkpoint.snapshot.transcript.entries.len(), 2);
    let rows = &checkpoint.snapshot.transcript.entries;
    let process = rows[0].turn_process.as_ref().unwrap();
    assert_eq!(rows[1].turn_process.as_ref(), Some(process));
    assert_eq!(process.conversation_id, origin);
    assert!(process.attempt_id.is_none());
    assert_eq!(process.message_count, 1);
    assert_eq!(process.tool_call_count, 1);
    assert_eq!(process.control_cursor.get(), 1);
    assert_eq!(
        process.outcome,
        crate::runtime_client::response::TurnProcessOutcome::Completed
    );
    assert!(rows[0].completed_response.is_none());
    let response = rows[1].completed_response.as_ref().unwrap();
    assert_eq!(response.origin.conversation_id, origin);
    assert!(response.origin.attempt_id.is_none());
    assert_eq!(
        response.closing_message_id.as_ref(),
        Some(&MessageId::new("owner-final"))
    );
    assert_eq!(
        process.final_message_locator.as_ref(),
        Some(&response.closing_locator)
    );
    read_original_owner(
        &host,
        &checkpoint,
        process.attempt_id_source.as_ref().unwrap(),
        InheritedAttemptIdentityKind::Turn,
        &original,
    );
    assert_eq!(response.origin.attempt_id_source, process.attempt_id_source);
    read_original_owner(
        &host,
        &checkpoint,
        response.origin.attempt_id_source.as_ref().unwrap(),
        InheritedAttemptIdentityKind::Turn,
        &original,
    );
    assert!(store.read_events(None, 32).unwrap().events.is_empty());
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    assert!(crate::runtime_client::checkpoint::bounded_bytes(&checkpoint, 786_432).is_some());
}

#[tokio::test]
async fn t08_t12_t14_t19_cold_host_response_only_giant_owner_uses_original_identity_ranges() {
    let (seed, origin, original) = giant_owner_seed(false);
    assert!(seed.turns().is_empty());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("response-only-owner.sqlite");
    let conversation = ConversationId::generate();
    {
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize_lineage(&seed).unwrap();
        assert!(owner.load_inherited_turns().unwrap().is_empty());
        assert_eq!(
            owner.load_inherited_responses().unwrap(),
            seed.completed_responses()
        );
        assert_eq!(owner.load_canonical().unwrap().as_slice(), seed.canonical());
    }
    let store =
        Arc::new(SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap());
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    assert_eq!(checkpoint.cursor.get(), 0);
    assert_eq!(checkpoint.read_cut.through, 0);
    assert_eq!(checkpoint.read_cut.conversation_id, conversation);
    assert!(
        checkpoint.snapshot.attempt.is_none(),
        "no local execution is invented"
    );
    let rows = &checkpoint.snapshot.transcript.entries;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|row| row.turn_process.is_none()),
        "no inherited Turn is invented"
    );
    assert!(rows[0].completed_response.is_none());
    let response = rows[1].completed_response.as_ref().unwrap();
    assert_eq!(response.origin.conversation_id, origin);
    assert!(response.origin.attempt_id.is_none());
    assert_eq!(
        response.closing_message_id.as_ref(),
        Some(&MessageId::new("owner-final"))
    );
    assert_eq!(
        response.origin.closing_message_id,
        response.closing_message_id
    );
    read_original_owner(
        &host,
        &checkpoint,
        response.origin.attempt_id_source.as_ref().unwrap(),
        InheritedAttemptIdentityKind::Response,
        &original,
    );
    assert!(store.read_events(None, 32).unwrap().events.is_empty());
    assert!(store.load_inherited_turns().unwrap().is_empty());
    assert_eq!(
        store.load_inherited_responses().unwrap(),
        seed.completed_responses()
    );
    assert!(crate::runtime_client::checkpoint::bounded_bytes(&checkpoint, 786_432).is_some());
}

#[tokio::test]
async fn t08_t12_t14_t19_cold_giant_turn_page_navigates_by_native_ordinal() {
    let (seed, origin, original) = giant_owner_seed(true);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("giant-turn-page.sqlite");
    let conversation = ConversationId::generate();
    {
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize_lineage(&seed).unwrap();
        assert_eq!(owner.load_inherited_turns().unwrap(), seed.turns());
        assert_eq!(owner.load_canonical().unwrap().as_slice(), seed.canonical());
    }
    let store =
        Arc::new(SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap());
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    let page = host.inner.conversation_turns(Some(0), 1).unwrap();
    assert_eq!(
        page.cut,
        checkpoint.read_cut.source_cut.as_ref().unwrap().read
    );
    assert_eq!((page.total, page.offset, page.turns.len()), (1, 0, 1));
    let turn = &page.turns[0];
    assert_eq!(turn.ordinal, 1);
    assert_eq!(turn.id.conversation_id, origin);
    assert!(turn.id.attempt_id.is_none());
    assert_eq!(turn.cursor.unwrap().get(), 1);
    assert_eq!(turn.prompt, "");
    assert_eq!(turn.response, "original final body");
    let process = checkpoint.snapshot.transcript.entries[0]
        .turn_process
        .as_ref()
        .unwrap();
    assert_eq!(turn.id.attempt_id_source, process.attempt_id_source);
    read_original_owner(
        &host,
        &checkpoint,
        turn.id.attempt_id_source.as_ref().unwrap(),
        InheritedAttemptIdentityKind::Turn,
        &original,
    );
    let at = crate::durable::reading::ConversationWindowAt::TurnOrdinal {
        ordinal: turn.ordinal,
        cut: page.cut.clone(),
    };
    let window = host.inner.conversation_window(&at, 2).unwrap();
    assert_eq!(window.cut, page.cut);
    assert_eq!(window.target.as_ref(), Some(&turn.id));
    assert_eq!(window.target_cursor, turn.cursor.map(Into::into));
    assert_eq!(window.page.entries.len(), 2);
    assert_eq!(window.page.entries[0].turn_process.as_ref(), Some(process));
    assert!(window.page.entries[0].completed_response.is_none());
    assert!(window.page.entries[1].completed_response.is_some());
    assert_eq!(host.checkpoint().unwrap(), checkpoint);
    assert_eq!(host.inner.conversation_turns(Some(0), 1).unwrap(), page);
    assert_eq!(host.inner.conversation_window(&at, 2).unwrap(), window);
    assert!(
        host.inner
            .conversation_window(
                &crate::durable::reading::ConversationWindowAt::TurnOrdinal {
                    ordinal: 0,
                    cut: page.cut.clone(),
                },
                2
            )
            .is_err()
    );
    let mut foreign = page.cut.clone();
    foreign.conversation_id = ConversationId::generate();
    assert!(
        host.inner
            .conversation_window(
                &crate::durable::reading::ConversationWindowAt::TurnOrdinal {
                    ordinal: turn.ordinal,
                    cut: foreign,
                },
                2
            )
            .is_err()
    );
    assert_eq!(host.checkpoint().unwrap(), checkpoint);
    assert!(checkpoint.snapshot.attempt.is_none());
    assert!(store.read_events(None, 32).unwrap().events.is_empty());
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    assert!(crate::runtime_client::checkpoint::bounded_bytes(&page, 65_536).is_some());
    assert!(crate::runtime_client::checkpoint::bounded_bytes(&window, 786_432).is_some());
}

fn read_local_owner(host: &RuntimeClientHost, checkpoint: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint, original: &str) {
    use base64::Engine as _;
    let owner = checkpoint.snapshot.attempt.as_ref().unwrap();
    let Some(AttemptIdentitySource::Journal {field, domain}) = &owner.attempt_id_source else {panic!("original native StartJ source")};
    assert_eq!(field.sequence, 1);
    assert_eq!(field.path, ["attempt_id"]);
    let expected = serde_json::to_vec(original).unwrap();
    assert_eq!(field.encoded_bytes, expected.len() as u64);
    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        let chunk = host.inner.checkpoint_value_read(&checkpoint.read_cut, domain, 0, &field.path, offset, 65_536).unwrap();
        assert_eq!(chunk.cut, checkpoint.read_cut);
        assert_eq!(chunk.path, field.path);
        assert_eq!(chunk.readable_end, field.encoded_bytes);
        let part = base64::engine::general_purpose::STANDARD.decode(chunk.bytes_base64).unwrap();
        assert!(!part.is_empty() && part.len() <= 65_536);
        bytes.extend(part);
        let Some(next) = chunk.next_offset else {break};
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(bytes, expected);
    assert_eq!(host.checkpoint().unwrap(), *checkpoint);
}

#[test]
fn t08_t12_t14_t19_cold_local_giant_j_owner_keeps_recorded_terminal_and_original_ranges() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("local-giant-j-owner.sqlite");
    let cid = ConversationId::generate();
    let original = "original-local-owner水🙂\\\"\n".repeat(80_000);
    assert!(original.len() > 1_048_576);
    let attempt_id = AttemptId::new(&original);
    let writer = SqliteConversationStore::open(cid.clone(), &path).unwrap();
    writer.initialize(&[]).unwrap();
    writer.append_event(durable_event(&cid, "giant-local-start", &attempt_id, RuntimeEvent::AttemptStarted {attempt_id: attempt_id.clone()})).unwrap();
    let started_cut = writer.observation_read_cut().unwrap();
    let started_store = Arc::new(SqliteConversationStore::open_existing(cid.clone(), &path).unwrap());
    let started_host = RuntimeClientHost::new_durable(started_store, None).unwrap();
    let started = started_host.checkpoint().unwrap();
    assert_eq!(started.read_cut.source_cut.as_ref(), Some(&started_cut));
    assert_eq!(started.cursor.get(), 0);
    assert!(started.snapshot.attempt.as_ref().unwrap().attempt_id.is_none());
    assert_eq!(started.snapshot.attempt.as_ref().unwrap().phase, RuntimeClientAttemptPhase::Incomplete);
    assert!(started_host.inner.runtime.is_none());
    read_local_owner(&started_host, &started, &original);
    let message = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {id: MessageId::new("local-giant-owner-result"), content: vec![AssistantContentBlock::Text(crate::message::content::TextBlock {text: "positive original final C".into()})]});
    writer.append_canonical_with_event(&message, durable_event(&cid, "giant-local-c", &attempt_id, RuntimeEvent::AssistantMessageCommitted {message_id: message.id().clone()})).unwrap();
    writer.append_event(durable_event(&cid, "giant-local-terminal", &attempt_id, RuntimeEvent::AttemptCompleted {attempt_id: attempt_id.clone(), finish_reason: ModelFinishReason::Stop})).unwrap();
    let final_cut = writer.observation_read_cut().unwrap();
    let original_j = writer.read_events(None, 32).unwrap().events;
    let original_c = writer.load_canonical().unwrap();
    let store = Arc::new(SqliteConversationStore::open_existing(cid, &path).unwrap());
    SqliteConversationStore::trace_read_work();
    let host = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    let (rows, bytes) = SqliteConversationStore::trace_read_work();
    assert!(rows > 0 && bytes < 262_144, "finite headers: {rows} rows/{bytes} bytes");
    assert_eq!(checkpoint.cursor.get(), 0);
    assert_eq!(checkpoint.read_cut.source_cut.as_ref(), Some(&final_cut));
    let owner = checkpoint.snapshot.attempt.as_ref().unwrap();
    assert!(owner.attempt_id.is_none());
    assert_eq!(owner.attempt_id_source, started.snapshot.attempt.as_ref().unwrap().attempt_id_source);
    assert!(matches!(owner.phase, RuntimeClientAttemptPhase::Settled {outcome: crate::runtime_client::event::RuntimeClientOutcome::Completed {finish_reason: ModelFinishReason::Stop, finish_reason_deferred: false, finish_reason_source: None}}));
    let entry = checkpoint.snapshot.transcript.entries.iter().find(|entry| entry.completed_response.is_some()).unwrap();
    assert_eq!(entry.turn_process.as_ref().unwrap().attempt_id_source, owner.attempt_id_source);
    assert_eq!(entry.completed_response.as_ref().unwrap().origin.attempt_id_source, owner.attempt_id_source);
    read_local_owner(&host, &checkpoint, &original);
    assert_eq!(started_host.checkpoint().unwrap(), started);
    assert_eq!(store.read_events(None, 32).unwrap().events, original_j);
    assert_eq!(store.load_canonical().unwrap(), original_c);
    assert!(host.inner.runtime.is_none());
}

#[test]
fn t08_t14_t19_standalone_trace_keeps_the_attached_constructor_cut_after_later_c_and_j() {
    let writer = Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
    writer.initialize(&[]).unwrap();
    let original = "standalone-native-owner水\"\n".repeat(30_000);
    let attempt = AttemptId::new(&original);
    writer.append_event(durable_event(writer.conversation_id(), "standalone-owner-start", &attempt, RuntimeEvent::AttemptStarted {attempt_id: attempt.clone()})).unwrap();
    let host = RuntimeClientHost::new_durable(writer.clone(), None).unwrap();
    let checkpoint = host.checkpoint().unwrap();
    let (attachment, _) = host.attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION).unwrap();
    let subscription = attachment.subscribe_events(checkpoint.cursor).unwrap();
    let RuntimeClientResult::TracePage {page: before} = host.inner.trace_page(None, 32, Vec::new()).unwrap() else {panic!("native Trace page")};
    assert_eq!(before.read_cut.as_ref(), Some(&checkpoint.read_cut));
    assert_eq!(before.records.len(), 1);
    assert_eq!(before.records[0].state, crate::runtime_client::trace::TraceState::Incomplete);
    let message = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {id: MessageId::new("standalone-later-c"), content: vec![AssistantContentBlock::Text(crate::message::content::TextBlock {text: "later original C".into()})]});
    writer.append_canonical_with_event(&message, durable_event(writer.conversation_id(), "standalone-later-j", &attempt, RuntimeEvent::AssistantMessageCommitted {message_id: message.id().clone()})).unwrap();
    let RuntimeClientResult::TracePage {page: after} = host.inner.trace_page(None, 32, vec![before.records[0].position.clone()]).unwrap() else {panic!("native Trace page")};
    assert_eq!(after.read_cut, before.read_cut);
    assert_eq!(after.records, before.records);
    assert_eq!(after.updates.len(), 1);
    assert_eq!(after.updates[0].state, crate::runtime_client::trace::TraceState::Incomplete);
    let returned = after.read_cut.unwrap();
    assert_eq!(returned, checkpoint.read_cut);
    let mut selected_checkpoint = checkpoint.clone();
    selected_checkpoint.read_cut = returned;
    read_local_owner(&host, &selected_checkpoint, &original);
    assert_eq!(host.checkpoint().unwrap(), checkpoint);
    assert!(matches!(subscription.try_next(), EventDelivery::Pending));
    let fresh = RuntimeClientHost::new_durable(writer.clone(), None).unwrap();
    let RuntimeClientResult::TracePage {page: current} = fresh.inner.trace_page(None, 32, Vec::new()).unwrap() else {panic!("new native Trace owner")};
    assert_eq!(current.records.len(), 2);
    let fresh_cut = current.read_cut.unwrap();
    assert_eq!(fresh_cut.source_cut.as_ref(), Some(&writer.observation_read_cut().unwrap()));
    assert!(fresh_cut.through > checkpoint.read_cut.through);
    assert!(fresh_cut.source_cut.unwrap().ledger_through > checkpoint.read_cut.source_cut.unwrap().ledger_through);
    assert!(matches!(subscription.try_next(), EventDelivery::Pending));
}
