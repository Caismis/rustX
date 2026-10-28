//! Exact C occurrence plus bounded Tool execution evidence from its native J.
use super::{
    Connection, ConversationId, ConversationStoreError, OptionalExtension, params, storage,
};
use crate::durable::observation::{
    NativeToolObservationHeader, NativeToolObservationPage, ObservationReadCut,
};
use crate::runtime_client::content::CanonicalContentLocator;

/// Exact inputs already encoded by the native page's mandatory admission pass.
/// The cut is deliberately separate: the transcript reader records the actual
/// admitted prefix after applying its current-cut metadata and byte budget.
#[derive(PartialEq, Eq)]
pub(super) struct PageProof {
    message_id: Option<crate::runtime::identity::MessageId>,
    locator: CanonicalContentLocator,
    total: u64,
    offset: u64,
    next_offset: Option<u64>,
    headers: Vec<Vec<u8>>,
}
impl PageProof {
    pub(super) fn retain_prefix(&mut self, loaded: usize) {
        self.headers.truncate(loaded);
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.headers.iter().map(Vec::len).sum::<usize>()
            + self.message_id.as_ref().map_or(0, |id| id.as_str().len())
            + match &self.locator {
                CanonicalContentLocator::ByMessageId { message_id } => message_id.as_str().len(),
                CanonicalContentLocator::ByLedgerPosition { .. } => 0,
            }
    }
}

pub(super) fn page(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    locator: &CanonicalContentLocator,
    offset: u64,
    limit: usize,
) -> Result<NativeToolObservationPage, ConversationStoreError> {
    let page = super::observation::tools_by_locator(
        connection,
        conversation,
        cut,
        locator,
        offset,
        limit,
    )?;
    page_at(connection, conversation, cut, locator, page, None)
}

pub(super) fn page_validated(
    validated: &super::observation::ValidatedCut<'_, '_>,
    locator: &CanonicalContentLocator,
    offset: u64,
    limit: usize,
) -> Result<(NativeToolObservationPage, PageProof), ConversationStoreError> {
    let connection = validated.connection()?;
    let cut = validated.expected();
    let page = super::observation::tools_by_locator_validated(validated, locator, offset, limit)?;
    let mut headers = Vec::new();
    let page = page_at(
        connection,
        &cut.read.conversation_id,
        cut,
        locator,
        page,
        Some(&mut headers),
    )?;
    let proof = PageProof {
        message_id: page.message_id.clone(),
        locator: page.locator.clone(),
        total: page.total,
        offset: page.offset,
        next_offset: page.next_offset,
        headers,
    };
    Ok((page, proof))
}

fn page_at(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    locator: &CanonicalContentLocator,
    page: crate::durable::observation::ToolAssociationHeaderPage,
    mut proof: Option<&mut Vec<Vec<u8>>>,
) -> Result<NativeToolObservationPage, ConversationStoreError> {
    let ledger = super::content::canonical_position(connection, locator)?;
    let mut remaining = 786_432usize - 1024;
    let mut next_offset = page.next_offset;
    let mut headers = Vec::new();
    for mut association in page.headers {
        let block = association.block_index.get();
        let name = connection.query_row("SELECT observation_name FROM canonical_tool_calls INDEXED BY canonical_tool_calls_observation_idx WHERE observation_ledger=?1 AND block_index=?2",params![ledger,block],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        let argument_ref = argument_reference(connection, conversation, ledger, block)?;
        if let Some(reference) = &argument_ref
            && !association
                .call
                .content
                .iter()
                .any(|binding| binding.content_ref == *reference)
        {
            association
                .call
                .content
                .push(crate::runtime_client::checkpoint::ContentBinding {
                    source_path: None,
                    path: vec!["value".into(), "arguments".into()],
                    content_ref: reference.clone(),
                    deferred: reference.readable_end > 1024,
                });
        }
        let header = NativeToolObservationHeader {
            name,
            arguments_json: arguments(connection, conversation, argument_ref.as_ref())?,
            committed_result: association
                .result
                .as_ref()
                .map(|result| {
                    committed_result(
                        connection,
                        result
                            .ledger_position
                            .ok_or_else(|| storage("Tool result lacks native ledger position"))?,
                    )
                })
                .transpose()?,
            started: witness(connection, cut, ledger, block, 1)?,
            terminal: witness(connection, cut, ledger, block, 2)?,
            association,
        };
        #[cfg(test)]
        PAGE_WORK.with(|work| work.set(work.get() + 1));
        let Some(bytes) = crate::runtime_client::checkpoint::bounded_bytes(&header, remaining)
        else {
            next_offset = Some(u64::from(block));
            break;
        };
        remaining -= bytes.len();
        if let Some(proof) = &mut proof {
            proof.push(bytes);
        }
        headers.push(header);
    }
    if headers.is_empty() && next_offset.is_some() {
        return Err(storage("native Tool observation page cannot advance"));
    }
    Ok(NativeToolObservationPage {
        cut: page.cut,
        message_id: page.message_id,
        locator: page.locator,
        total: page.total,
        offset: page.offset,
        headers,
        next_offset,
    })
}

#[cfg(test)]
thread_local! {
    static PAGE_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
/// Mandatory native Tool header encodes, shared by public and proven reads.
#[cfg(test)]
pub(super) fn page_work() -> usize {
    PAGE_WORK.with(|work| work.replace(0))
}
fn witness(
    connection: &Connection,
    cut: &ObservationReadCut,
    ledger: u64,
    block: u32,
    phase: u8,
) -> Result<Option<crate::durable::observation::ObservationEventWitness>, ConversationStoreError> {
    let sequence: Option<u64> = connection.query_row(
        "SELECT sequence FROM events INDEXED BY events_observation_tool_lifecycle_idx WHERE observation_tool_phase IS NOT NULL AND observation_tool_ledger=?1 AND observation_tool_block=?2 AND observation_tool_phase=?3 AND sequence<=?4 ORDER BY sequence DESC LIMIT 1",
        params![ledger,block,phase,cut.read.journal], |row| row.get(0),
    ).optional().map_err(|error| storage(error.to_string()))?;
    sequence
        .map(|sequence| {
            super::observation_event::witness(connection, sequence, cut.read.journal)?
                .ok_or_else(|| storage("native Tool execution witness unavailable"))
        })
        .transpose()
}

pub(super) fn result_header(
    result: &crate::tools::types::ToolExecutionResult,
) -> crate::durable::observation::NativeToolResultHeader {
    let complete = crate::runtime_client::checkpoint::bounded_clone(result, 14_336);
    crate::durable::observation::NativeToolResultHeader {
        content_total: result.content.len() as u64,
        result_deferred: complete.is_none(),
        result: complete.unwrap_or_else(|| crate::tools::types::ToolExecutionResult {
            workflow: None,
            status: super::observation_receipt::positive_status(&result.status),
            content: Vec::new(),
            duration_ms: result.duration_ms,
            exit_code: result.exit_code,
            artifacts: Vec::new(),
            deliveries: Vec::new(),
            truncation: result.truncation.clone(),
            managed_output: None,
        }),
    }
}
fn committed_result(
    connection: &Connection,
    ledger: u64,
) -> Result<crate::durable::observation::NativeToolResultHeader, ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row(
            "SELECT tool_result_json FROM observation_headers WHERE ledger_position=?1",
            [ledger],
            |row| row.get(0),
        )
        .map_err(|error| storage(error.to_string()))?;
    let bytes = bytes.ok_or_else(|| storage("native Tool result summary unavailable"))?;
    if bytes.len() > 16_384 {
        return Err(storage("oversized native Tool result summary"));
    }
    serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
}

pub(super) fn name_preview(name: &str) -> &str {
    let mut end = name.len().min(256);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

fn argument_reference(
    connection: &Connection,
    conversation: &ConversationId,
    ledger: u64,
    block: u32,
) -> Result<Option<crate::runtime_client::content::ContentRef>, ConversationStoreError> {
    use crate::runtime_client::content::{
        CanonicalContentComponent, ContentRepresentation, ContentSource,
    };
    let source = ContentSource::CanonicalMessageBlock {
        locator: CanonicalContentLocator::ByLedgerPosition { position: ledger },
        block_index: crate::message::types::ContentBlockIndex::new(block),
        component: CanonicalContentComponent::Payload,
    };
    let raw = super::content::descriptor(
        connection,
        conversation,
        &source,
        ContentRepresentation::RawJsonUtf8,
    )?;
    let reference = match raw.filter(|reference| reference.readable_end > 0) {
        Some(raw) => Some(raw),
        None => super::content::descriptor(
            connection,
            conversation,
            &source,
            ContentRepresentation::CanonicalJsonV1,
        )?,
    };
    Ok(reference)
}
fn arguments(
    connection: &Connection,
    conversation: &ConversationId,
    reference: Option<&crate::runtime_client::content::ContentRef>,
) -> Result<Option<String>, ConversationStoreError> {
    let Some(reference) = reference.filter(|reference| reference.readable_end <= 1024) else {
        return Ok(None);
    };
    let chunk = super::content::read(connection, conversation, reference, 0, 1024)?;
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        chunk.data_base64,
    )
    .map_err(|error| storage(error.to_string()))?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| storage(error.to_string()))
}
