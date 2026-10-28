//! Reuse only one transaction's successful original descriptor capture. The
//! original body and every public byte budget still have one authoritative path.
#[cfg(test)]
use super::PUBLICATION_CAPTURE_WORK;
use super::{
    Connection, ConversationStoreError, HEADER_ENCODED_MAX, MessageBlock, MessageBlockHeader,
    OptionalExtension, PublicationHeader, block_bytes, identity, message_header_bytes, params,
    seq_to_i64, storage,
};
use crate::durable::sqlite::statements::FixedStatements;

impl PublicationHeader {
    pub(in crate::durable::sqlite) fn rebind(
        mut self,
        connection: &Connection,
        message: &MessageBlock,
        position: u64,
    ) -> Result<(), ConversationStoreError> {
        let MessageBlock::Assistant(assistant) = message else {
            return Err(storage("publication display must be an Assistant"));
        };
        if self.header.ledger_position != Some(position) {
            return Err(storage(
                "publication display changed its native Ledger owner",
            ));
        }
        let conversation = identity(connection)?;
        // Only Tool argument descriptors change at raw retention. All other
        // block bodies and metadata were stored by the same successful capture.
        for (index, block) in assistant.content.iter().enumerate() {
            if matches!(
                block,
                crate::message::types::AssistantContentBlock::ToolCall(_)
            ) {
                rebind_tool_block(connection, message, index)?;
            }
        }
        for binding in self.header.content.iter_mut().take(self.captured_bindings) {
            if binding
                .path
                .last()
                .is_some_and(|field| field == "arguments")
            {
                let raw = super::super::content::descriptor(
                    connection,
                    &conversation,
                    &binding.content_ref.source,
                    crate::runtime_client::content::ContentRepresentation::RawJsonUtf8,
                )?;
                // Match canonical_bindings' original eager integrity check of
                // the structural representation even when raw bytes exist.
                let structural = super::super::content::descriptor(
                    connection,
                    &conversation,
                    &binding.content_ref.source,
                    binding.content_ref.representation,
                )?;
                binding.content_ref = raw
                    .or(structural)
                    .ok_or_else(|| storage("canonical descriptor missing native byte index"))?;
            }
        }
        self.header.transcript_cursor = connection.query_row_cached(
            "SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1",
            [message.id().as_str()], |row| row.get::<_, u64>(0),
        ).optional().map_err(|error| storage(error.to_string()))?.map(crate::durable::TranscriptCursor::new);
        let bytes = message_header_bytes(&mut self.header)?;
        let actual: Option<Vec<u8>> = connection.query_row_cached(
            "SELECT header_json FROM observation_headers WHERE reference_kind='message' AND reference_id=?1 AND ledger_position=?2",
            params![message.id().as_str(),position], |row|row.get(0),
        ).optional().map_err(|error|storage(error.to_string()))?;
        if actual.as_deref() != Some(bytes.as_slice()) {
            // This also repairs a failed/over-budget T patch. Use the same
            // upsert as the original final capture, with its exact typed budget.
            connection.execute_cached("INSERT INTO observation_headers(reference_kind,reference_id,ledger_position,header_json,transcript_position) VALUES('message',?1,?2,?3,(SELECT position FROM transcript_order WHERE reference_kind='message' AND reference_id=?1)) ON CONFLICT(reference_kind,reference_id) DO UPDATE SET ledger_position=excluded.ledger_position,header_json=excluded.header_json",params![message.id().as_str(),seq_to_i64(position)?,bytes]).map_err(|error|storage(error.to_string()))?;
        }
        #[cfg(test)]
        PUBLICATION_CAPTURE_WORK.with(|work| {
            let (messages, blocks, reused) = work.get();
            work.set((messages, blocks, reused + 1));
        });
        Ok(())
    }
}

fn rebind_tool_block(
    connection: &Connection,
    message: &MessageBlock,
    index: usize,
) -> Result<(), ConversationStoreError> {
    let bytes: Vec<u8> = connection.query_row_cached(
        "SELECT header_json FROM observation_block_headers WHERE message_id=?1 AND block_index=?2",
        params![message.id().as_str(), index], |row| row.get(0),
    ).map_err(|error| storage(error.to_string()))?;
    if bytes.len() > HEADER_ENCODED_MAX {
        return Err(storage("oversized canonical block header"));
    }
    let header: MessageBlockHeader = serde_json::from_slice(&bytes)
        .map_err(|_| storage("canonical publication block header corrupt"))?;
    if usize::try_from(header.block_index.get()).ok() != Some(index) || header.kind != "tool_call" {
        return Err(storage("canonical publication block owner differs"));
    }
    // Original admitted value is unchanged; only final physical descriptors
    // need rebinding. The shared builder preserves order, deferred markers,
    // raw spelling, metadata fallback and the complete 16 KiB admission.
    let rebound = block_bytes(
        connection,
        message,
        index,
        "tool_call",
        header.value,
        &["arguments"],
    )?;
    if rebound != bytes {
        connection.execute_cached(
            "UPDATE observation_block_headers SET header_json=?3 WHERE message_id=?1 AND block_index=?2",
            params![message.id().as_str(), index, rebound],
        ).map_err(|error| storage(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
