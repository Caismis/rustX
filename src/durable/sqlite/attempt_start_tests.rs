fn candidates(store: &SqliteConversationStore) -> [RuntimeEventEnvelope; 2] {
    let attempt = AttemptId::new("atomic-first-turn");
    let started = envelope(
        store.conversation_id(),
        "attempt-start",
        Some(attempt.clone()),
        RuntimeEvent::AttemptStarted { attempt_id: attempt.clone() },
    );
    let mut turn = envelope(store.conversation_id(), "turn-start", Some(attempt), RuntimeEvent::TurnStarted);
    turn.turn_id = Some(TurnId::new("1"));
    [started, turn]
}

fn admission_state(store: &SqliteConversationStore) -> (u64, u64, u64, u64) {
    store.lock().unwrap().query_row(
        "SELECT (SELECT count(*) FROM events),(SELECT count(*) FROM lifecycle_state),(SELECT count(*) FROM event_observation_readiness),next_event_sequence FROM rustx_store WHERE id=1",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    ).unwrap()
}

#[test]
fn attempt_first_turn_commits_once_with_one_authenticated_observation_family() {
    let store = store();
    store.initialize(&[]).unwrap();
    let commits = Arc::new(AtomicUsize::new(0));
    store.lock().unwrap().commit_hook(Some({
        let commits = commits.clone();
        move || { commits.fetch_add(1, Ordering::SeqCst); false }
    })).unwrap();
    let [attempt, turn] = candidates(&store);
    let [attempt, turn] = store.commit_attempt_first_turn(attempt, turn).unwrap();
    store.lock().unwrap().commit_hook(None::<fn() -> bool>).unwrap();
    assert_eq!(commits.load(Ordering::SeqCst), 1);
    assert_eq!((attempt.sequence, turn.sequence), (1, 2));
    let batch = attempt.observation.as_ref().unwrap();
    assert!(Arc::ptr_eq(batch, turn.observation.as_ref().unwrap()));
    assert_eq!(batch.journal_range, Some((1, 2)));
    assert_eq!(batch.read_cut.read.journal, 2);
    assert!(!batch.source_unavailable);
    let crate::durable::observation::NativeObservationTransition::AttemptFirstTurn { attempt: first, turn: second } = &batch.transition else { panic!("fixed admission family") };
    for (event, witness) in [(&attempt, first), (&turn, second)] {
        let stored = store.observation_event_witness(event.sequence, 2).unwrap().unwrap();
        assert_eq!(serde_json::to_vec(witness).unwrap(), serde_json::to_vec(&stored).unwrap());
    }
    assert_eq!(admission_state(&store), (2, 2, 2, 2));
    let [attempt, turn] = candidates(&store);
    assert!(matches!(store.commit_attempt_first_turn(attempt, turn), Err(ConversationStoreError::TerminalViolation(_))));
    assert_eq!(admission_state(&store), (2, 2, 2, 2));
}

#[test]
fn attempt_first_turn_rejects_foreign_or_nonfirst_members_without_writes() {
    let store = store();
    store.initialize(&[]).unwrap();
    let before = admission_state(&store);
    for malformed in 0..7 {
        let [mut attempt, mut turn] = candidates(&store);
        match malformed {
            0 => turn.conversation_id = ConversationId::generate(),
            1 => turn.attempt_id = Some(AttemptId::new("foreign")),
            2 => attempt.attempt_id = None,
            3 => attempt.turn_id = Some(TurnId::new("1")),
            4 => turn.turn_id = Some(TurnId::new("2")),
            5 => turn.event = RuntimeEvent::TurnCompleted,
            6 => attempt.event = RuntimeEvent::TurnStarted,
            _ => unreachable!(),
        }
        assert!(matches!(store.commit_attempt_first_turn(attempt, turn), Err(ConversationStoreError::InvalidReference(_))));
        assert_eq!(admission_state(&store), before);
    }
}

#[test]
fn attempt_first_turn_second_member_failure_rolls_back_all_admission_evidence() {
    let store = store();
    store.initialize(&[]).unwrap();
    let before = admission_state(&store);
    store.lock().unwrap().execute_batch(
        "CREATE TRIGGER reject_first_turn BEFORE INSERT ON events WHEN NEW.turn_id='1' BEGIN SELECT RAISE(ABORT,'reject second admission member'); END;"
    ).unwrap();
    let [attempt, turn] = candidates(&store);
    assert!(matches!(store.commit_attempt_first_turn(attempt, turn), Err(ConversationStoreError::Storage(_))));
    assert_eq!(admission_state(&store), before);
    assert!(store.read_events(None, 8).unwrap().events.is_empty());
    store.lock().unwrap().execute_batch("DROP TRIGGER reject_first_turn;").unwrap();
    let [attempt, turn] = candidates(&store);
    assert_eq!(store.commit_attempt_first_turn(attempt, turn).unwrap()[1].sequence, 2);
}

#[test]
fn attempt_first_turn_corrupt_member_fences_both_receipts_without_losing_native_events() {
    for target in [1, 2] {
        let store = store();
        store.initialize(&[]).unwrap();
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER damage_admission_witness AFTER UPDATE OF observation_witness ON events WHEN NEW.sequence=2 BEGIN UPDATE events SET observation_witness=X'5b' WHERE sequence={target}; END;"
        )).unwrap();
        let [attempt, turn] = candidates(&store);
        let events = store.commit_attempt_first_turn(attempt, turn).unwrap();
        assert!(events.iter().all(|event| event.observation.is_none()));
        assert_eq!(store.read_events(None, 8).unwrap().events.len(), 2);
        assert_eq!(store.observation_ready_frontier(0, 2).unwrap(), 0);
    }
}
