use super::ConversationStoreError;
use crate::durable::observation::AttemptMemberCounts;
use crate::durable::sqlite::observation_codec as codec;

type Record = (
    u64,
    u64,
    Option<crate::durable::TranscriptCursor>,
    Option<crate::runtime::identity::MessageId>,
    Option<crate::runtime_client::content::CanonicalContentLocator>,
    bool,
    Option<crate::runtime_client::content::ContentRef>,
    bool,
    Option<crate::durable::observation::MemberProcessHeader>,
);

pub(super) fn encode(value: &AttemptMemberCounts) -> Result<Vec<u8>, ConversationStoreError> {
    codec::encode(
        value,
        &(
            value.message_count,
            value.tool_call_count,
            value.first_cursor,
            &value.final_message_id,
            &value.final_message_locator,
            value.final_message_id_deferred,
            &value.final_message_id_source,
            value.completed_response,
            &value.process,
        ),
        crate::durable::observation::HEADER_ENCODED_MAX,
    )
}

pub(super) fn decode(bytes: &[u8]) -> Result<AttemptMemberCounts, ConversationStoreError> {
    let (
        message_count,
        tool_call_count,
        first_cursor,
        final_message_id,
        final_message_locator,
        final_message_id_deferred,
        final_message_id_source,
        completed_response,
        process,
    ): Record = codec::decode(bytes)?;
    let value = AttemptMemberCounts {
        message_count,
        tool_call_count,
        first_cursor,
        final_message_id,
        final_message_locator,
        final_message_id_deferred,
        final_message_id_source,
        completed_response,
        process,
    };
    codec::validate_public(&value, crate::durable::observation::HEADER_ENCODED_MAX)?;
    Ok(value)
}
