use super::{ConversationStoreError, ObservationEventWitness};
use crate::durable::sqlite::observation_codec as codec;

type Record = (
    Option<crate::durable::observation::OccupancyObservation>,
    bool,
    Option<u64>,
    bool,
    u64,
    Option<crate::durable::TranscriptCursor>,
    Option<crate::runtime::identity::EventId>,
    Option<crate::runtime::identity::AttemptId>,
    Option<crate::runtime::identity::TurnId>,
    chrono::DateTime<chrono::Utc>,
    String,
    Option<crate::events::types::ToolObservationOccurrence>,
    Option<u64>,
    serde_json::Value,
    Vec<crate::durable::observation::ObservationEventField>,
    bool,
);

fn fields(value: &ObservationEventWitness) -> impl serde::Serialize + '_ {
    (
        &value.occupancy,
        value.native_direct,
        value.trace_total,
        value.source_unavailable,
        value.sequence,
        value.transcript_cursor,
        &value.event_id,
        &value.attempt_id,
        &value.turn_id,
        &value.timestamp,
        &value.event_kind,
        &value.tool_occurrence,
        value.result_ledger_position,
        &value.value,
        &value.fields,
        value.deferred,
    )
}

pub(super) fn encode(value: &ObservationEventWitness) -> Result<Vec<u8>, ConversationStoreError> {
    codec::encode(value, &fields(value), 16384 - 128)
}

pub(super) fn attach(
    value: &mut ObservationEventWitness,
    cursor: crate::durable::TranscriptCursor,
) -> Result<Vec<u8>, ConversationStoreError> {
    // The original json_set guard applied to JSON bytes before the T patch.
    codec::public_length(value, 16384 - 128)?;
    value.transcript_cursor = Some(cursor);
    codec::public_length(value, 16384)?;
    codec::encode_admitted(&fields(value))
}

pub(super) fn decode(bytes: &[u8]) -> Result<ObservationEventWitness, ConversationStoreError> {
    let (
        occupancy,
        native_direct,
        trace_total,
        source_unavailable,
        sequence,
        transcript_cursor,
        event_id,
        attempt_id,
        turn_id,
        timestamp,
        event_kind,
        tool_occurrence,
        result_ledger_position,
        value,
        fields,
        deferred,
    ): Record = codec::decode(bytes)?;
    let witness = ObservationEventWitness {
        occupancy,
        native_direct,
        trace_total,
        source_unavailable,
        sequence,
        transcript_cursor,
        event_id,
        attempt_id,
        turn_id,
        timestamp,
        event_kind,
        tool_occurrence,
        result_ledger_position,
        value,
        fields,
        deferred,
    };
    codec::validate_public(&witness, 16384)?;
    Ok(witness)
}
