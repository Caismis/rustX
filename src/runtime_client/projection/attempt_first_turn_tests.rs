use crate::durable::ConversationStore;
use crate::durable::observation::NativeObservationTransition;

#[test]
fn attempt_first_turn_family_preserves_ordered_facts_old_cut_and_single_publication() {
    let (mut actual, store) = initialized_source_projection();
    let mut expected = projection();
    expected
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    expected.initialize_publication_view();
    let mut start = source_tool_event(
        &store,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
        "family-attempt",
    );
    start.turn_id = None;
    let [start, turn] = store
        .commit_attempt_first_turn(
            start,
            source_tool_event(&store, RuntimeEvent::TurnStarted, "family-first-turn"),
        )
        .unwrap();
    let shared = start.observation.as_ref().unwrap();
    assert!(Arc::ptr_eq(shared, turn.observation.as_ref().unwrap()));
    let family = shared.as_ref();
    assert_eq!(family.journal_range, Some((start.sequence, turn.sequence)));
    let NativeObservationTransition::AttemptFirstTurn {
        attempt: attempt_witness,
        turn: turn_witness,
    } = &family.transition
    else {
        panic!("fixed attempt/first-turn family");
    };
    let old = actual.checkpoint_seed();
    let old_checkpoint = old.checkpoint_candidate().unwrap();
    let cursor = actual.cursor;
    publish_source_fact(&mut actual, |projection| {
        projection.apply_native_batch(family);
    });
    publish_source_fact(&mut expected, |projection| {
        // The former per-event fold is an independent ordered oracle inside
        // one outer publication, preserving cursor and cut comparability.
        for witness in [attempt_witness, turn_witness] {
            let mut member = family.clone();
            member.transition = NativeObservationTransition::Single(witness.as_ref().clone());
            member.journal_range = Some((witness.sequence, witness.sequence));
            projection.apply_native_batch(&member);
        }
    });
    assert_eq!(actual.cursor.get(), cursor.get() + 1);
    assert_eq!(actual.snapshot, expected.snapshot);
    assert_eq!(
        actual.replay.back().unwrap().1.facts(),
        expected.replay.back().unwrap().1.facts()
    );
    assert_eq!(actual.snapshot.attempt.as_ref().unwrap().turn, 1);
    let facts = actual.replay.back().unwrap().1.facts();
    let started = facts
        .iter()
        .position(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
        .unwrap();
    let turned = facts
        .iter()
        .position(|fact| matches!(fact, RuntimeClientEvent::AttemptTurnUpdated { turn: 1, .. }))
        .unwrap();
    assert!(started < turned);
    assert_eq!(old.checkpoint_candidate().unwrap(), old_checkpoint);
    actual.validate_source_prefix().unwrap();
    store.mark_native_observation_ready(family).unwrap();
    let published = actual.cursor;
    let witnesses = [start.sequence, turn.sequence]
        .into_iter()
        .map(|sequence| {
            let witness = store
                .observation_event_witness(sequence, turn.sequence)
                .unwrap()
                .unwrap();
            assert!(witness.native_direct);
            ConversationObservation::JournalFact(witness)
        })
        .collect();
    publish_source_fact(&mut actual, |projection| {
        projection.apply(ConversationObservation::JournalBatch {
            through: Some(turn.sequence),
            observations: witnesses,
        });
    });
    assert_eq!(
        actual.cursor, published,
        "worker replay cannot republish the installed family"
    );
    assert_eq!(actual.snapshot.attempt.as_ref().unwrap().turn, 1);
}
