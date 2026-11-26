use super::ConversationStoreError;
use crate::observation::AttemptMemberCounts;
use crate::sqlite::observation_codec as codec;

type Record = (
    u64,
    u64,
    Option<crate::TranscriptCursor>,
    Option<rustx_types::runtime::identity::MessageId>,
    Option<rustx_types::observation::content::CanonicalContentLocator>,
    bool,
    Option<rustx_types::observation::content::ContentRef>,
    bool,
    Option<crate::observation::MemberProcessHeader>,
);

pub fn encode(value: &AttemptMemberCounts) -> Result<Vec<u8>, ConversationStoreError> {
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
        crate::observation::HEADER_ENCODED_MAX,
    )
}

pub fn decode(bytes: &[u8]) -> Result<AttemptMemberCounts, ConversationStoreError> {
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
    codec::validate_public(&value, crate::observation::HEADER_ENCODED_MAX)?;
    Ok(value)
}
