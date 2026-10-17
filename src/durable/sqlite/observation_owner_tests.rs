// Owner displays select immutable original fields, never nullable aliases.
fn owner_seed(attempt: &str, with_turn: bool) -> LineageSeed {
    let original = member_completed_lineage("owner-final");
    let mut turns = original.turns().to_vec();
    turns[0].id.attempt_id = AttemptId::new(attempt);
    let mut responses = original.completed_responses().to_vec();
    responses[0].origin.attempt_id = AttemptId::new(attempt);
    LineageSeed::history(original.canonical().to_vec())
        .with_turns(if with_turn { turns } else { Vec::new() })
        .unwrap()
        .with_completed_responses(responses)
        .unwrap()
}
fn original_owner_field(
    store: &SqliteConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
    source: &crate::runtime_client::response::AttemptIdentitySource,
) -> String {
    use crate::runtime_client::response::AttemptIdentitySource;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let chunk = match source {
            AttemptIdentitySource::Journal { field, .. } => store
                .event_value_read(
                    field.sequence,
                    cut.read.journal,
                    &field.path,
                    offset,
                    65_536,
                )
                .unwrap(),
            AttemptIdentitySource::Inherited { kind, ordinal, .. } => store
                .observation_inherited_attempt_read(cut, *kind, *ordinal, offset, 65_536)
                .unwrap(),
        };
        assert!(chunk.bytes.len() <= 65_536);
        if matches!(source, AttemptIdentitySource::Inherited { .. }) {
            let (rows, materialized) = SqliteConversationStore::content_read_work();
            assert!(rows <= 2, "finite segment count: {rows}");
            assert!(materialized <= 131_072);
        }
        bytes.extend(chunk.bytes);
        match chunk.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    serde_json::from_slice(&bytes).unwrap()
}
fn owner_completed_view(
    store: &SqliteConversationStore,
    cut: &crate::durable::observation::ObservationReadCut,
) -> crate::runtime_client::response::CompletedResponseView {
    member_source_page(store, cut)
        .entries
        .into_iter()
        .find_map(|entry| entry.completed_response)
        .unwrap()
}
#[test]
fn t08_t12_t14_t19_giant_local_owner_keeps_one_original_j_selector_at_every_cut() {
    let store = store();
    store.initialize(&[]).unwrap();
    let original = format!("\u{feff}{}", "owner🙂水\"\\\n".repeat(90_000));
    let attempt = AttemptId::new(&original);
    let start = store
        .append_event(envelope(
            store.conversation_id(),
            "owner-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    store
        .append_canonical_with_event(
            &assistant_message("owner-final", "native final"),
            envelope(
                store.conversation_id(),
                "owner-c",
                Some(attempt.clone()),
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: MessageId::new("owner-final"),
                },
            ),
        )
        .unwrap();
    let before = store.observation_read_cut().unwrap();
    let running = member_source_page(&store, &before).entries[0]
        .turn_process
        .clone()
        .unwrap();
    assert!(running.attempt_id.is_none());
    assert_eq!(
        running.outcome,
        crate::durable::reading::TurnProcessOutcome::Running
    );
    let source = running.attempt_id_source.as_ref().unwrap();
    assert!(
        matches!(source,crate::runtime_client::response::AttemptIdentitySource::Journal{field,..} if field.sequence==start.sequence)
    );
    assert_eq!(original_owner_field(&store, &before, source), original);
    store
        .append_event(envelope(
            store.conversation_id(),
            "owner-stop",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt,
                finish_reason: ModelFinishReason::Stop,
            },
        ))
        .unwrap();
    let after = store.observation_read_cut().unwrap();
    let page = member_source_page(&store, &after);
    let process = page.entries[0].turn_process.as_ref().unwrap();
    let response = page.entries[0].completed_response.as_ref().unwrap();
    assert_eq!(process.attempt_id_source, running.attempt_id_source);
    assert_eq!(response.origin.attempt_id_source, process.attempt_id_source);
    assert!(response.origin.attempt_id.is_none());
    assert!(serde_json::to_vec(&page).unwrap().len() < 65_536);
    assert_eq!(
        member_source_page(&store, &before).entries[0].turn_process,
        Some(running)
    );
}
#[test]
fn t08_t12_t14_t19_giant_inherited_owner_and_response_only_owner_have_real_ordinals() {
    use crate::runtime_client::response::{
        AttemptIdentitySource, InheritedAttemptIdentityKind as Kind,
    };
    let original = "inherited-owner🙂水".repeat(70_000);
    for with_turn in [true, false] {
        let store = store();
        let seed = owner_seed(&original, with_turn);
        store.initialize_lineage(&seed).unwrap();
        assert_eq!(
            store.load_inherited_responses().unwrap(),
            seed.completed_responses()
        );
        let cut = store.observation_read_cut().unwrap();
        let response = owner_completed_view(&store, &cut);
        assert!(response.origin.attempt_id.is_none());
        let source = response.origin.attempt_id_source.as_ref().unwrap();
        assert!(
            matches!(source,AttemptIdentitySource::Inherited{kind,ordinal:0,..} if *kind==if with_turn{Kind::Turn}else{Kind::Response})
        );
        assert_eq!(original_owner_field(&store, &cut, source), original);
        assert!(serde_json::to_vec(&response).unwrap().len() < 16_384);
        if with_turn {
            let process = member_source_page(&store, &cut).entries[1]
                .turn_process
                .clone()
                .unwrap();
            assert_eq!(process.attempt_id_source, response.origin.attempt_id_source);
        }
    }
}
#[test]
fn t10_t14_t19_provenance_display_failure_preserves_native_bootstrap_and_owner_field() {
    let store = store();
    store.lock().unwrap().execute_batch("CREATE TRIGGER owner_display_fault BEFORE UPDATE OF display_provenance ON inherited_turn_provenance BEGIN SELECT RAISE(FAIL,'derived owner display'); END; CREATE TRIGGER owner_turn_page_fault BEFORE UPDATE OF observation_json ON inherited_turn_provenance BEGIN SELECT RAISE(FAIL,'derived turn page'); END;").unwrap();
    let seed = owner_seed("native-provenance-owner", true);
    store.initialize_lineage(&seed).unwrap();
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    assert_eq!(
        store.load_inherited_responses().unwrap(),
        seed.completed_responses()
    );
    assert_eq!(store.load_canonical().unwrap().as_slice(), seed.canonical());
    let cut = store.observation_read_cut().unwrap();
    let process = member_source_page(&store, &cut).entries[1]
        .turn_process
        .clone()
        .unwrap();
    assert_eq!(
        process.attempt_id,
        Some(AttemptId::new("native-provenance-owner"))
    );
    let original_source = super::observation_identity::inherited(
        &store.lock().unwrap(),
        crate::runtime_client::response::InheritedAttemptIdentityKind::Turn,
        0,
    )
    .unwrap();
    assert_eq!(
        original_owner_field(&store, &cut, &original_source),
        "native-provenance-owner"
    );
    assert!(store.conversation_turns(0, 32).is_err());
}
#[test]
fn t10_t14_t19_provenance_owner_index_hole_is_observation_only() {
    let store = store();
    store.lock().unwrap().execute_batch("CREATE TRIGGER owner_field_fault BEFORE INSERT ON content_frontiers WHEN NEW.source_kind='inherited_attempt_identity' BEGIN SELECT RAISE(FAIL,'derived owner field'); END;").unwrap();
    let original = "owner-field🙂".repeat(100_000);
    let seed = owner_seed(&original, true);
    store.initialize_lineage(&seed).unwrap();
    assert_eq!(store.load_inherited_turns().unwrap(), seed.turns());
    assert_eq!(
        store.load_inherited_responses().unwrap(),
        seed.completed_responses()
    );
    let cut = store.observation_read_cut().unwrap();
    let error = store.observation_transcript_process(&cut, 2).unwrap_err();
    assert!(matches!(
        crate::runtime_client::RuntimeClientError::observation_store_error(error),
        crate::runtime_client::RuntimeClientError::ObservationUnavailable { .. }
    ));
}

#[test]
fn t08_t14_t19_giant_terminal_owner_remains_positive_without_a_content_member() {
    let store = store();
    store.initialize(&[]).unwrap();
    let original = "terminal-owner🙂".repeat(80_000);
    let attempt = AttemptId::new(&original);
    let start = store
        .append_event(envelope(
            store.conversation_id(),
            "terminal-owner-start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let cancelled = store
        .append_event(envelope(
            store.conversation_id(),
            "terminal-owner-cancel",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCancelled {
                attempt_id: attempt,
                reason: crate::runtime::types::CancellationReason::UserRequested,
            },
        ))
        .unwrap();
    let cut = store.observation_read_cut().unwrap();
    let page = member_source_page(&store, &cut);
    let crate::runtime_client::snapshot::RuntimeClientTranscriptItem::AttemptTerminal { turn } =
        &page.entries[0].item
    else {
        panic!("original terminal must remain positive")
    };
    assert_eq!(
        turn.outcome,
        crate::durable::reading::TurnProcessOutcome::Cancelled
    );
    assert_eq!(turn.event_id, Some(cancelled.event_id));
    assert_eq!(turn.message_count, 0);
    assert!(turn.attempt_id.is_none());
    let source = turn.attempt_id_source.as_ref().unwrap();
    assert!(
        matches!(source,crate::runtime_client::response::AttemptIdentitySource::Journal{field,..} if field.sequence==start.sequence)
    );
    assert_eq!(original_owner_field(&store, &cut, source), original);
    assert!(serde_json::to_vec(turn).unwrap().len() < 16_384);
}
#[test]
fn t08_t12_t19_response_only_repeated_owner_selects_first_real_provenance() {
    let original = "response-owner🙂水".repeat(70_000);
    let seed = owner_seed(&original, false);
    let mut canonical = seed.canonical().to_vec();
    canonical.push(assistant_message(
        "second-owner-final",
        "second actual response",
    ));
    let mut responses = seed.completed_responses().to_vec();
    let mut second = responses[0].clone();
    second.process_message_ids = vec![MessageId::new("second-owner-final")];
    second.closing_message_id = MessageId::new("second-owner-final");
    second.origin.closing_message_id = second.closing_message_id.clone();
    responses.push(second);
    let seed = LineageSeed::history(canonical)
        .with_completed_responses(responses)
        .unwrap();
    let store = store();
    store.initialize_lineage(&seed).unwrap();
    let cut = store.observation_read_cut().unwrap();
    let page = member_source_page(&store, &cut);
    let views: Vec<_> = page
        .entries
        .iter()
        .filter_map(|entry| entry.completed_response.as_ref())
        .collect();
    assert_eq!(views.len(), 2);
    assert_eq!(
        views[0].origin.attempt_id_source,
        views[1].origin.attempt_id_source
    );
    assert!(matches!(
        views[0].origin.attempt_id_source,
        Some(
            crate::runtime_client::response::AttemptIdentitySource::Inherited {
                kind: crate::runtime_client::response::InheritedAttemptIdentityKind::Response,
                ordinal: 0,
                ..
            }
        )
    ));
    assert_eq!(
        original_owner_field(
            &store,
            &cut,
            views[0].origin.attempt_id_source.as_ref().unwrap()
        ),
        original
    );
}

#[test]
fn t08_t12_t19_inherited_owner_selected_field_enforces_original_cut_and_range() {
    use crate::runtime_client::RuntimeClientError;
    use crate::runtime_client::response::AttemptIdentitySource;
    let store = store();
    let original = "selected-owner🙂".repeat(80_000);
    store
        .initialize_lineage(&owner_seed(&original, true))
        .unwrap();
    let cut = crate::runtime_client::trace_observation::history_cut(&store).unwrap();
    let response = owner_completed_view(&store, cut.source_cut.as_ref().unwrap());
    let AttemptIdentitySource::Inherited {
        domain,
        encoded_bytes,
        ..
    } = response.origin.attempt_id_source.unwrap()
    else {
        panic!("actual inherited source")
    };
    let path = vec!["attempt_id".into()];
    let good =
        crate::runtime_client::attempt_identity::value_read(&store, &cut, &domain, 0, &path, 0, 3)
            .unwrap();
    assert_eq!(good.readable_end, encoded_bytes);
    assert_eq!(good.next_offset, Some(3));
    assert!(matches!(
        crate::runtime_client::attempt_identity::value_read(
            &store,
            &cut,
            &domain,
            0,
            &["unknown".into()],
            0,
            10
        ),
        Err(RuntimeClientError::InvalidRequest { .. })
    ));
    assert!(matches!(
        crate::runtime_client::attempt_identity::value_read(&store, &cut, &domain, 1, &path, 0, 10),
        Err(RuntimeClientError::InvalidRequest { .. })
    ));
    assert!(matches!(
        crate::runtime_client::attempt_identity::value_read(
            &store,
            &cut,
            &domain,
            0,
            &path,
            encoded_bytes + 1,
            10
        ),
        Err(RuntimeClientError::InvalidRange)
    ));
    let mut foreign = cut.clone();
    foreign.conversation_id = ConversationId::generate();
    assert!(matches!(
        crate::runtime_client::attempt_identity::value_read(
            &store, &foreign, &domain, 0, &path, 0, 10
        ),
        Err(RuntimeClientError::ContentSourceUnavailable)
    ));
    let mut forged = cut.clone();
    forged.through += 1;
    assert!(matches!(
        crate::runtime_client::attempt_identity::value_read(
            &store, &forged, &domain, 0, &path, 0, 10
        ),
        Err(RuntimeClientError::ReadCutExpired)
    ));
}
