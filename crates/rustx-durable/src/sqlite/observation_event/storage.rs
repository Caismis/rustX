use super::{ConversationStoreError, ObservationEventWitness};
use crate::sqlite::observation_codec as codec;

type Record = (
    Option<crate::observation::OccupancyObservation>,
    bool,
    Option<u64>,
    bool,
    u64,
    Option<crate::TranscriptCursor>,
    Option<rustx_types::runtime::identity::EventId>,
    Option<rustx_types::runtime::identity::AttemptId>,
    Option<rustx_types::runtime::identity::TurnId>,
    chrono::DateTime<chrono::Utc>,
    (
        String,
        Option<crate::observation::RequestOwnerKind>,
        Option<rustx_types::runtime::identity::AuxiliaryCallId>,
    ),
    Option<rustx_types::events::types::ToolObservationOccurrence>,
    Option<u64>,
    serde_json::Value,
    Vec<crate::observation::ObservationEventField>,
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
        (&value.event_kind, &value.owner, &value.auxiliary_call_id),
        &value.tool_occurrence,
        value.result_ledger_position,
        &value.value,
        &value.fields,
        value.deferred,
    )
}

pub fn encode(value: &ObservationEventWitness) -> Result<Vec<u8>, ConversationStoreError> {
    codec::encode(value, &fields(value), 16384 - 128)
}

pub fn attach(
    value: &mut ObservationEventWitness,
    cursor: crate::TranscriptCursor,
) -> Result<Vec<u8>, ConversationStoreError> {
    // The original json_set guard applied to JSON bytes before the T patch.
    codec::public_length(value, 16384 - 128)?;
    value.transcript_cursor = Some(cursor);
    codec::public_length(value, 16384)?;
    codec::encode_admitted(&fields(value))
}

pub fn decode(bytes: &[u8]) -> Result<ObservationEventWitness, ConversationStoreError> {
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
        (event_kind, owner, auxiliary_call_id),
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
        owner,
        auxiliary_call_id,
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
