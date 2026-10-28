use super::{ConversationStoreError, TraceFactHeader};
use crate::durable::sqlite::observation_codec as codec;

type Record = (
    u64,
    chrono::DateTime<chrono::Utc>,
    crate::durable::observation::TraceFactKind,
    Option<u64>,
    crate::runtime_client::trace::TraceLocation,
    Vec<crate::runtime_client::trace::TraceFieldSource>,
    crate::durable::observation::TraceFactData,
    Option<crate::durable::observation::TraceBootstrapFact>,
    Option<bool>,
);

pub(super) fn encode(value: &TraceFactHeader) -> Result<Vec<u8>, ConversationStoreError> {
    codec::encode(
        value,
        &(
            value.sequence,
            &value.timestamp,
            &value.kind,
            value.owner_anchor,
            &value.location,
            &value.fields,
            &value.fact,
            &value.bootstrap,
            value.canonical_fields_ready,
        ),
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
}

pub(super) fn decode(bytes: &[u8]) -> Result<TraceFactHeader, ConversationStoreError> {
    let (
        sequence,
        timestamp,
        kind,
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
        owner_anchor,
        location,
        fields,
        fact,
        bootstrap,
        canonical_fields_ready,
    };
    codec::validate_public(&value, crate::durable::observation::HEADER_ENCODED_MAX)?;
    Ok(value)
}
