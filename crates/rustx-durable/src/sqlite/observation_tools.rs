//! Exact C occurrence plus bounded Tool execution evidence from its native J.
use super::{
    Connection, ConversationId, ConversationStoreError, OptionalExtension, params, storage,
};
use crate::observation::{
    NativeToolObservationHeader, NativeToolObservationPage, ObservationReadCut,
};
use crate::sqlite::statements::FixedStatements as _;
use rustx_types::observation::content::CanonicalContentLocator;

pub const TOOL_NAME_SQL: &str = "SELECT observation_name FROM canonical_tool_calls INDEXED BY canonical_tool_calls_observation_idx WHERE observation_ledger=?1 AND block_index=?2";
pub const TOOL_WITNESS_SQL: &str = "SELECT sequence FROM events INDEXED BY events_observation_tool_lifecycle_idx WHERE observation_tool_phase IS NOT NULL AND observation_tool_ledger=?1 AND observation_tool_block=?2 AND observation_tool_phase=?3 AND (?3!=1 OR observation_kind='tool_execution_started') AND sequence<=?4 ORDER BY sequence DESC LIMIT 1";
pub const TOOL_RESULT_SQL: &str =
    "SELECT tool_result_json FROM transcript_entries WHERE ledger_position=?1";

/// Exact inputs already encoded by the native page's mandatory admission pass.
/// The cut is deliberately separate: the transcript reader records the actual
/// admitted prefix after applying its current-cut metadata and byte budget.
#[derive(PartialEq, Eq)]
pub struct PageProof {
    message_id: Option<rustx_types::runtime::identity::MessageId>,
    locator: CanonicalContentLocator,
    total: u64,
    offset: u64,
    next_offset: Option<u64>,
    headers: Vec<Vec<u8>>,
}
impl PageProof {
    pub fn retain_prefix(&mut self, loaded: usize) {
        self.headers.truncate(loaded);
    }
    pub fn retained_bytes(&self) -> usize {
        self.headers.iter().map(Vec::len).sum::<usize>()
            + self.message_id.as_ref().map_or(0, |id| id.as_str().len())
            + match &self.locator {
                CanonicalContentLocator::ByMessageId { message_id } => message_id.as_str().len(),
                CanonicalContentLocator::ByLedgerPosition { .. } => 0,
            }
    }
}

pub fn page(
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
    #[cfg(any(test, feature = "test-hooks"))]
    AFTER_ASSOCIATIONS.with(|probe| {
        if let Some(probe) = probe.borrow_mut().take() {
            probe();
        }
    });
    page_at(connection, conversation, cut, page, None)
}

pub fn page_validated(
    validated: &super::observation::ValidatedCut<'_, '_>,
    locator: &CanonicalContentLocator,
    offset: u64,
    limit: usize,
    known: Option<(&[u8], &super::observation::ToolOwner)>,
) -> Result<(NativeToolObservationPage, PageProof), ConversationStoreError> {
    let connection = validated.connection()?;
    let cut = validated.expected();
    let page =
        super::observation::tools_by_locator_validated(validated, locator, offset, limit, known)?;
    let mut headers = Vec::new();
    let page = page_at(
        connection,
        &cut.read.conversation_id,
        cut,
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
    page: crate::observation::ToolAssociationHeaderPage,
    mut proof: Option<&mut Vec<Vec<u8>>>,
) -> Result<NativeToolObservationPage, ConversationStoreError> {
    // tools_by_locator_at just resolved this locator in the same read
    // transaction; no writes occur between that lookup and these native reads.
    let CanonicalContentLocator::ByLedgerPosition { position: ledger } = &page.locator else {
        return Err(storage("native Tool page lacks resolved ledger position"));
    };
    let ledger = *ledger;
    let mut remaining = 786_432usize - 1024;
    let mut next_offset = page.next_offset;
    let mut headers = Vec::new();
    for mut association in page.headers {
        let block = association.block_index.get();
        let name = connection
            .query_row_cached(TOOL_NAME_SQL, params![ledger, block], |row| row.get(0))
            .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
        let argument_ref = argument_reference(connection, conversation, ledger, block)?;
        if let Some(reference) = &argument_ref
            && !association
                .call
                .content
                .iter()
                .any(|binding| binding.content_ref.as_ref() == reference)
        {
            association
                .call
                .content
                .push(crate::observation::cut::ContentBinding {
                    source_path: None,
                    path: vec!["value".into(), "arguments".into()],
                    content_ref: reference.clone().into(),
                    deferred: reference.readable_end > 1024,
                });
        }
        let header = NativeToolObservationHeader {
            name,
            arguments_json: arguments(connection, conversation, argument_ref.as_ref(), ledger)?,
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
        #[cfg(any(test, feature = "test-hooks"))]
        PAGE_WORK.with(|work| work.set(work.get() + 1));
        let Some(bytes) = rustx_types::observation::bounded::bounded_bytes(&header, remaining)
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

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static PAGE_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static AFTER_ASSOCIATIONS: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(any(test, feature = "test-hooks"))]
pub fn probe_after_associations(probe: impl FnOnce() + 'static) {
    AFTER_ASSOCIATIONS.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
}
/// Mandatory native Tool header encodes, shared by public and proven reads.
#[cfg(any(test, feature = "test-hooks"))]
pub fn page_work() -> usize {
    PAGE_WORK.with(|work| work.replace(0))
}
fn witness(
    connection: &Connection,
    cut: &ObservationReadCut,
    ledger: u64,
    block: u32,
    phase: u8,
) -> Result<Option<crate::observation::ObservationEventWitness>, ConversationStoreError> {
    let sequence: Option<u64> = connection
        .query_row_cached(
            TOOL_WITNESS_SQL,
            params![ledger, block, phase, cut.read.journal],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    sequence
        .map(|sequence| {
            super::observation_event::witness(connection, sequence, cut.read.journal)?
                .ok_or_else(|| storage("native Tool execution witness unavailable"))
        })
        .transpose()
}

#[must_use]
pub fn result_header(
    result: &rustx_types::tools::types::ToolExecutionResult,
) -> crate::observation::NativeToolResultHeader {
    let complete = crate::observation::bounded::bounded_clone(result, 14_336);
    crate::observation::NativeToolResultHeader {
        content_total: result.content.len() as u64,
        result_deferred: complete.is_none(),
        result: complete.unwrap_or_else(|| rustx_types::tools::types::ToolExecutionResult {
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
) -> Result<crate::observation::NativeToolResultHeader, ConversationStoreError> {
    let bytes: Option<Vec<u8>> = connection
        .query_row_cached(TOOL_RESULT_SQL, [ledger], |row| row.get(0))
        .map_err(|error| crate::sqlite::sqlite_error(&error, error.to_string()))?;
    let bytes = bytes.ok_or_else(|| storage("native Tool result summary unavailable"))?;
    if bytes.len() > 16_384 {
        return Err(storage("oversized native Tool result summary"));
    }
    serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
}

#[must_use]
pub fn name_preview(name: &str) -> &str {
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
) -> Result<Option<rustx_types::observation::content::ContentRef>, ConversationStoreError> {
    use rustx_types::observation::content::{
        CanonicalContentComponent, ContentRepresentation, ContentSource,
    };
    let source = ContentSource::CanonicalMessageBlock {
        locator: CanonicalContentLocator::ByLedgerPosition { position: ledger },
        block_index: rustx_types::message::types::ContentBlockIndex::new(block),
        component: CanonicalContentComponent::Payload,
    };
    let raw = crate::sqlite::content::canonical_descriptor_at_position(
        connection,
        conversation,
        &source,
        ContentRepresentation::RawJsonUtf8,
        ledger,
    )?;
    let reference = match raw.filter(|reference| reference.readable_end > 0) {
        Some(raw) => Some(raw),
        None => crate::sqlite::content::canonical_descriptor_at_position(
            connection,
            conversation,
            &source,
            ContentRepresentation::CanonicalJsonV1,
            ledger,
        )?,
    };
    Ok(reference)
}
fn arguments(
    connection: &Connection,
    conversation: &ConversationId,
    reference: Option<&rustx_types::observation::content::ContentRef>,
    ledger: u64,
) -> Result<Option<String>, ConversationStoreError> {
    let Some(reference) = reference.filter(|reference| reference.readable_end <= 1024) else {
        return Ok(None);
    };
    let (bytes, _) = crate::sqlite::content::canonical_read_bytes_at_position(
        connection,
        conversation,
        reference,
        0,
        1024,
        ledger,
    )?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| storage(error.to_string()))
}
