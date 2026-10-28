//! The fixed admission boundary before an attempt prepares its first turn.

use super::{
    ConversationStoreError, RuntimeEvent, RuntimeEventEnvelope, SqliteConversationStore,
    TransactionBehavior, observation_receipt, persist_event_tx_deferred, process_death, storage,
};

pub(super) fn commit(
    store: &SqliteConversationStore,
    attempt: RuntimeEventEnvelope,
    turn: RuntimeEventEnvelope,
) -> Result<[RuntimeEventEnvelope; 2], ConversationStoreError> {
    let RuntimeEvent::AttemptStarted { attempt_id } = &attempt.event else {
        return Err(storage_identity());
    };
    if !matches!(turn.event, RuntimeEvent::TurnStarted)
        || attempt.attempt_id.as_ref() != Some(attempt_id)
        || turn.attempt_id.as_ref() != Some(attempt_id)
        || attempt.turn_id.is_some()
        || turn
            .turn_id
            .as_ref()
            .map(crate::runtime::identity::TurnId::as_str)
            != Some("1")
        || attempt.conversation_id != turn.conversation_id
    {
        return Err(storage_identity());
    }
    let mut connection = store.lock()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| storage(format!("attempt and first turn transaction: {error}")))?;
    #[cfg(test)]
    if SqliteConversationStore::consume(&store.fail_event_remaining) {
        return Err(storage("fault injected: event commit"));
    }
    process_death::reach_event("before:event", &attempt.event);
    let mut attempt = persist_event_tx_deferred(
        &store.conn.event_guard,
        &transaction,
        &store.conversation_id,
        attempt,
    )?;
    process_death::reach("during:commit_attempt_first_turn");
    process_death::reach_event("before:event", &turn.event);
    let mut turn = persist_event_tx_deferred(
        &store.conn.event_guard,
        &transaction,
        &store.conversation_id,
        turn,
    )?;
    let receipt = observation_receipt::attempt_first_turn(
        &transaction,
        &store.conversation_id,
        (&attempt.event, Some(&attempt.facts)),
        (&turn.event, Some(&turn.facts)),
    );
    attempt.event.observation.clone_from(&receipt);
    turn.event.observation = receipt;
    transaction
        .commit()
        .map_err(|error| storage(format!("attempt and first turn commit: {error}")))?;
    // Both old after-event hooks describe committed native evidence. A death
    // at either hook sees the complete pair, never a half-admitted attempt.
    process_death::reach_event("after:event", &attempt.event.event);
    process_death::reach_event("after:event", &turn.event.event);
    Ok([attempt.event, turn.event])
}

fn storage_identity() -> ConversationStoreError {
    ConversationStoreError::InvalidReference(
        "attempt admission requires its matching AttemptStarted and first TurnStarted".into(),
    )
}
