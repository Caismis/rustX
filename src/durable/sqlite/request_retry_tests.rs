use crate::model::projected::{
    CanonicalMessageRef, DerivedModelMessage, FrozenModelInput, ProjectedMessage,
    SemanticTransformation,
};

fn adopt_retry_message(
    store: &SqliteConversationStore,
    message: &MessageBlock,
    attempt: Option<&str>,
) {
    let MessageBlock::User(user) = message else {
        unreachable!()
    };
    let accepted = store
        .accept_inbound(crate::durable::InboundDraft {
            message_id: Some(user.id.clone()),
            source: user.source.clone(),
            kind: user.kind.clone(),
            content: user.content.clone(),
            timestamp: user.timestamp.unwrap(),
            correlation: None,
        })
        .unwrap();
    store
        .adopt_pending_batch(accepted.sequence, attempt.map(AttemptId::new))
        .unwrap();
}

fn start_retry_attempt(store: &SqliteConversationStore, id: &str) {
    store
        .append_event(envelope(
            store.conversation_id(),
            &format!("start-{id}"),
            Some(AttemptId::new(id)),
            RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new(id),
            },
        ))
        .unwrap();
}

fn uploaded_retry_fixture(id: &str) -> (SqliteConversationStore, RequestSnapshot, MessageBlock) {
    let store = store();
    let mut original = user_message(id, "Regenerate my uploaded note");
    let file = crate::message::content::UploadedFileRef {
        batch_id: "original-native-batch".into(),
        name: "note.txt".into(),
    };
    let MessageBlock::User(user) = &mut original else {
        unreachable!()
    };
    user.content
        .insert(0, UserContentBlock::UploadedFile(file.clone()));
    store.initialize(&[]).unwrap();
    adopt_retry_message(&store, &original, None);
    start_retry_attempt(&store, "attempt-1");
    let history = store.projected_history().unwrap();
    let mut messages = vec![crate::model::ModelInputMessage::Derived(
        DerivedModelMessage {
            message: ProjectedMessage::from(&original),
            sources: vec![CanonicalMessageRef::of(store.conversation_id(), &original)],
            checkpoint_sources: Vec::new(),
            transformations: Vec::new(),
        },
    )];
    crate::model::uploads::UploadProjection {
        files: vec![crate::model::uploads::ResolvedUpload {
            file,
            path: "/original-session/uploads/note.txt".into(),
        }],
    }
    .apply(&mut messages)
    .unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    snapshot.history_cut = history.cut;
    snapshot
        .freeze_input(
            store.conversation_id(),
            &ModelRequest {
                continuation_source: None,
                images: crate::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                messages: messages.into(),
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
            },
        )
        .unwrap();
    (store, snapshot, original)
}

#[test]
fn uploaded_frozen_request_retains_the_exact_original_retry_boundary() {
    let (store, snapshot, original) = uploaded_retry_fixture("original-uploaded-input");
    let FrozenModelInput::Derived(derived) = &snapshot.input[0] else {
        panic!("the actual upload renderer freezes an identity-free input");
    };
    assert_eq!(
        derived.transformations,
        [SemanticTransformation::UploadRendering]
    );
    assert!(derived.sources[0].matches(&original));
    let before = store.presentation_frontier().unwrap();
    let committed = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let through = committed.started.sequence;
    assert_eq!(through, before + 1);
    assert_eq!(
        store
            .observation_request_retry_at(through, through)
            .unwrap(),
        Some(original.id().clone())
    );
    assert_eq!(
        store
            .observation_request_retry_locator_at(through, through)
            .unwrap(),
        Some(
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                position: 1
            }
        )
    );
    let reconstructed = store
        .reconstruct_model_request(&snapshot.request_id)
        .unwrap();
    assert_eq!(
        reconstructed.messages.to_vec(),
        [crate::model::ModelInputMessage::Derived(derived.clone())]
    );
    assert_eq!(store.load_canonical().unwrap(), [original]);
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    let replay = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(replay.started, committed.started);
    assert_eq!(store.presentation_frontier().unwrap(), through);
}

#[test]
fn retry_opening_source_keeps_native_frozen_input_validation() {
    let (store, snapshot, _) = uploaded_retry_fixture("original-uploaded-input");
    let before = store.presentation_frontier().unwrap();
    let mut forged = snapshot;
    let FrozenModelInput::Derived(input) = &mut forged.input[0] else {
        unreachable!()
    };
    input.sources[0].content_digest = "0".repeat(64);
    assert!(matches!(
        store.commit_model_turn_start(&[], &forged, Utc::now()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(store.presentation_frontier().unwrap(), before);
}

#[test]
fn uploaded_retry_deferred_identity_uses_the_same_original_anchor_as_its_locator() {
    let id = "original-upload-".repeat(200);
    let (store, snapshot, original) = uploaded_retry_fixture(&id);
    let committed = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    let sequence = committed.started.sequence;
    assert_eq!(
        store
            .observation_request_retry_at(sequence, sequence)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .observation_request_retry_locator_at(sequence, sequence)
            .unwrap(),
        Some(
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                position: 1
            }
        )
    );
    let connection = store.lock().unwrap();
    let field = content::utf8_field_read(
        &connection,
        "response_identity",
        &format!("request-start:{sequence}"),
        1,
        0,
        16_384,
    )
    .unwrap();
    assert_eq!(field.bytes, original.id().as_str().as_bytes());
    assert_eq!(field.readable_end, id.len() as u64);
    assert_eq!(field.next_offset, None);
    drop(connection);
    assert_eq!(store.presentation_frontier().unwrap(), sequence);
    let mut completed = envelope(
        store.conversation_id(),
        "first-upload-request-completed",
        Some(snapshot.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            request_id: snapshot.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    completed.turn_id = Some(snapshot.identity.turn.clone());
    store.append_event(completed).unwrap();
    adopt_retry_message(
        &store,
        &user_message("later-steering", "Later question"),
        Some("attempt-1"),
    );
    let mut later = progress_request_snapshot(
        "attempt-1",
        "2",
        0,
        store.load_head().unwrap().revision,
        vec![],
    );
    freeze_test_input(&store, &mut later, &[]);
    let second = store
        .commit_model_turn_start(&[], &later, Utc::now())
        .unwrap()
        .started;
    assert_eq!(
        store
            .observation_request_retry_at(second.sequence, second.sequence)
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .observation_request_retry_locator_at(second.sequence, second.sequence)
            .unwrap(),
        store
            .observation_request_retry_locator_at(sequence, sequence)
            .unwrap()
    );
    let connection = store.lock().unwrap();
    for start in [sequence, second.sequence] {
        let field = content::utf8_field_read(
            &connection,
            "response_identity",
            &format!("request-start:{start}"),
            1,
            0,
            16_384,
        )
        .unwrap();
        assert_eq!(field.bytes, id.as_bytes());
        assert_eq!(field.next_offset, None);
    }
}

#[test]
fn retry_eligibility_uses_original_adoption_and_native_user_classification() {
    for mode in [
        "human",
        "agent",
        "external",
        "runtime",
        "fleet",
        "whitespace",
        "media",
        "unadopted",
        "goal",
    ] {
        let store = store();
        store.initialize(&[]).unwrap();
        let mut message = user_message("selected-input", "Opening\u{2003}question");
        let MessageBlock::User(user) = &mut message else {
            unreachable!()
        };
        user.source = match mode {
            "agent" => UserSource::Agent {
                agent_id: crate::runtime::identity::AgentId::new("parent"),
            },
            "external" => UserSource::ExternalSystem,
            "runtime" => UserSource::Runtime,
            "fleet" => UserSource::Fleet,
            _ => UserSource::Human,
        };
        if mode == "whitespace" {
            user.content = vec![UserContentBlock::Text(TextBlock {
                text: "\u{2003}\n\t\u{a0}".into(),
            })];
        } else if mode == "media" {
            user.content = vec![UserContentBlock::UploadedFile(
                crate::message::content::UploadedFileRef {
                    batch_id: "text-free-original".into(),
                    name: "only-file.txt".into(),
                },
            )];
        } else if mode == "goal" {
            user.kind = InboundKind::GoalContinuation(crate::goal::GoalRef {
                id: "original-goal".into(),
                revision: 1,
            });
        }
        if matches!(mode, "unadopted" | "goal") {
            store.append_canonical(&message).unwrap();
        } else {
            adopt_retry_message(&store, &message, None);
        }
        start_retry_attempt(&store, "attempt-1");
        let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
        freeze_test_input(&store, &mut snapshot, &[]);
        let started = store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started;
        let expected = matches!(mode, "human" | "agent").then(|| message.id().clone());
        assert_eq!(
            store
                .observation_request_retry_at(started.sequence, started.sequence)
                .unwrap(),
            expected,
            "{mode}"
        );
    }
}

#[test]
fn retry_capture_ignores_optional_display_holes_and_other_attempt_steering() {
    let (store, snapshot, original) = uploaded_retry_fixture("opening");
    let connection = store.lock().unwrap();
    assert_eq!(
        connection
            .execute(
                "DELETE FROM observation_headers WHERE reference_id=?1",
                [original.id().as_str()],
            )
            .unwrap(),
        1
    );
    drop(connection);
    let committed = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(
        store.load_request_snapshot(&snapshot.request_id).unwrap(),
        snapshot
    );
    let captured: Option<String> = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT retry_message_id FROM request_snapshots WHERE request_id=?1",
            [snapshot.request_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(captured.as_deref(), Some(original.id().as_str()));
    // This C-header hole belongs to the selected retry identity, not the
    // RequestStarted event's independent finite observation witness.
    let sequence = committed.started.sequence;
    assert!(
        matches!(store.observation_request_retry_at(sequence, sequence),
        Err(ConversationStoreError::Storage(message)) if message == "native retry message header unavailable")
    );
    assert_eq!(
        store
            .observation_request_retry_locator_at(sequence, sequence)
            .unwrap(),
        Some(
            crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition {
                position: 1
            }
        )
    );
    assert_eq!(store.presentation_frontier().unwrap(), sequence);
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM observation_headers WHERE reference_id=?1",
                [original.id().as_str()],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0,
        "native admission never repairs the missing optional header"
    );
    start_retry_attempt(&store, "selected-later");
    adopt_retry_message(
        &store,
        &user_message("previous-steering", "Previous Attempt only"),
        Some("attempt-1"),
    );
    store
        .append_canonical(&user_message("current-context-only", "Not adopted"))
        .unwrap();
    let mut later = progress_request_snapshot(
        "selected-later",
        "1",
        0,
        store.load_head().unwrap().revision,
        vec![],
    );
    freeze_test_input(&store, &mut later, &[]);
    let started = store
        .commit_model_turn_start(&[], &later, Utc::now())
        .unwrap()
        .started;
    assert_eq!(
        store
            .observation_request_retry_locator_at(started.sequence, started.sequence)
            .unwrap(),
        None
    );
}

#[test]
fn a_projection_summary_without_adoption_cannot_invent_a_retry_opening() {
    let store = store();
    store
        .initialize(&[user_message("historical-human", "Only immutable history")])
        .unwrap();
    let candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        &store,
        "summary-no-adoption",
        1,
        "Summary of historical input",
    );
    let input = crate::scripted_suites::support::context::checkpoint_commit_fixture(
        &store, candidate, None,
    );
    store.commit_projection(input).unwrap();
    start_retry_attempt(&store, "attempt-1");
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    freeze_test_input(&store, &mut snapshot, &[]);
    assert!(snapshot.input.iter().any(|input| matches!(input, FrozenModelInput::Derived(value) if value.transformations.contains(&SemanticTransformation::Summary))));
    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap()
        .started;
    assert_eq!(
        store
            .observation_request_retry_locator_at(started.sequence, started.sequence)
            .unwrap(),
        None
    );
}
