use super::{ConversationStoreError, TraceFactHeader};
use crate::sqlite::observation_codec as codec;

type Record = (
    u64,
    chrono::DateTime<chrono::Utc>,
    (
        crate::observation::TraceFactKind,
        Option<crate::observation::RequestOwnerKind>,
    ),
    Option<u64>,
    crate::observation::trace::TraceLocation,
    Vec<crate::observation::trace::TraceFieldSource>,
    crate::observation::TraceFactData,
    Option<crate::observation::TraceBootstrapFact>,
    Option<bool>,
);

pub fn encode(value: &TraceFactHeader) -> Result<Vec<u8>, ConversationStoreError> {
    codec::encode(
        value,
        &(
            value.sequence,
            &value.timestamp,
            (&value.kind, &value.owner),
            value.owner_anchor,
            &value.location,
            &value.fields,
            &value.fact,
            &value.bootstrap,
            value.canonical_fields_ready,
        ),
        crate::observation::HEADER_ENCODED_MAX,
    )
}

pub fn decode(bytes: &[u8]) -> Result<TraceFactHeader, ConversationStoreError> {
    let (
        sequence,
        timestamp,
        (kind, owner),
        owner_anchor,
        location,
        fields,
        fact,
        bootstrap,
        canonical_fields_ready,
    ): Record = codec::decode(bytes)?;
    let value = TraceFactHeader {
        sequence,
        timestamp,
        kind,
        owner,
        owner_anchor,
        location,
        fields,
        fact,
        bootstrap,
        canonical_fields_ready,
    };
    codec::validate_public(&value, crate::observation::HEADER_ENCODED_MAX)?;
    Ok(value)
}
