//! One bounded response-decoration read. All reuse belongs to this transaction and cut.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::conversation::SurfaceRevision;
use crate::durable::observation::{
    AttemptExecutionSummary, AttemptMemberCounts, HEADER_PAGE_MAX, InheritedResponseHeader,
    ObservationReadCut, ResponseIdentityField, ResponseIdentitySource, ResponsePage,
    ResponsePageItem, ResponsePageSelection, ResponsePageSelectionKind,
};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime_client::content::CanonicalContentLocator;
use crate::runtime_client::response::{CompletedResponseView, TurnProcessView};
use std::collections::BTreeMap;

type Models = (u64, u64, Option<u64>);

struct Reader<'a> {
    connection: &'a Connection,
    conversation: &'a ConversationId,
    cut: &'a ObservationReadCut,
    executions: BTreeMap<u64, AttemptExecutionSummary>,
    members: BTreeMap<u64, AttemptMemberCounts>,
    processes: BTreeMap<u64, Option<TurnProcessView>>,
    inherited: BTreeMap<u64, Option<InheritedResponseHeader>>,
    responses: BTreeMap<(bool, u64), Option<CompletedResponseView>>,
    fields: BTreeMap<(bool, u64), Vec<ResponseIdentityField>>,
}

pub(super) fn read(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    selections: &[ResponsePageSelection],
) -> Result<ResponsePage, ConversationStoreError> {
    if selections.len() > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidReference(
            "response page exceeds native header limit".into(),
        ));
    }
    super::observation::validate(connection, conversation, cut)?;
    read_validated(connection, conversation, cut, selections)
}

/// The caller has already validated this exact cut on this same read transaction.
/// Used after transcript capture; it must never outlive that transaction.
pub(super) fn read_validated(
    connection: &Connection,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    selections: &[ResponsePageSelection],
) -> Result<ResponsePage, ConversationStoreError> {
    if selections.len() > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidReference(
            "response page exceeds native header limit".into(),
        ));
    }
    if selections
        .iter()
        .any(|selection| selection.cursor > cut.read.transcript)
    {
        return Err(ConversationStoreError::InvalidReference(
            "response source beyond native transcript cut".into(),
        ));
    }
    // Every selected row used the original source lookup, which refuses a hole
    // anywhere in this Journal prefix. The witness is identical for the whole page.
    if !selections.is_empty() {
        let missing: bool = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_member_holes_sequence_idx WHERE observation_members_required=1 AND observation_members IS NULL AND sequence<=?1)").and_then(|mut statement| statement.query_row([cut.read.journal], |row| row.get(0))).map_err(|error| storage(error.to_string()))?;
        if missing {
            return Err(storage("native response owner header unavailable"));
        }
    }
    let mut reader = Reader {
        connection,
        conversation,
        cut,
        executions: BTreeMap::new(),
        members: BTreeMap::new(),
        processes: BTreeMap::new(),
        inherited: BTreeMap::new(),
        responses: BTreeMap::new(),
        fields: BTreeMap::new(),
    };
    let items = selections
        .iter()
        .map(|selection| reader.item(selection))
        .collect::<Result<_, _>>()?;
    Ok(ResponsePage {
        items,
        statistics: super::observation_statistics::read(connection, cut.read.journal)?,
    })
}

fn source_key(source: &ResponseIdentitySource) -> (bool, u64) {
    match source {
        ResponseIdentitySource::Attempt { sequence } => (false, *sequence),
        ResponseIdentitySource::Inherited { ordinal } => (true, *ordinal),
    }
}

impl Reader<'_> {
    fn execution(
        &mut self,
        sequence: u64,
    ) -> Result<AttemptExecutionSummary, ConversationStoreError> {
        if let Some(summary) = self.executions.get(&sequence) {
            return Ok(summary.clone());
        }
        #[cfg(test)]
        READ_WORK.with(|work| {
            let (executions, members) = work.get();
            work.set((executions + 1, members));
        });
        let summary = super::observation_execution::summary_selected(
            self.connection,
            sequence,
            self.cut.read.journal,
        )?;
        self.executions.insert(sequence, summary.clone());
        Ok(summary)
    }

    fn members(&mut self, sequence: u64) -> Result<AttemptMemberCounts, ConversationStoreError> {
        if let Some(counts) = self.members.get(&sequence) {
            return Ok(counts.clone());
        }
        #[cfg(test)]
        READ_WORK.with(|work| {
            let (executions, members) = work.get();
            work.set((executions, members + 1));
        });
        let counts = super::observation_members::at_sequence(self.connection, sequence)?;
        self.members.insert(sequence, counts.clone());
        Ok(counts)
    }

    fn inherited(
        &mut self,
        ordinal: u64,
    ) -> Result<Option<InheritedResponseHeader>, ConversationStoreError> {
        if let Some(header) = self.inherited.get(&ordinal) {
            return Ok(header.clone());
        }
        let bytes: Option<Vec<u8>> = self.connection.prepare_cached("SELECT display_provenance FROM inherited_response_provenance WHERE ordinal=?1 AND closing_ledger_position<=?2").and_then(|mut statement| statement.query_row(params![ordinal, self.cut.ledger_through], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        let header: Option<InheritedResponseHeader> = bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string())))
            .transpose()?;
        self.inherited.insert(ordinal, header.clone());
        Ok(header)
    }

    fn source_at_position(
        &self,
        position: u64,
    ) -> Result<Option<ResponseIdentitySource>, ConversationStoreError> {
        let anchor: Option<Option<u64>> = self.connection.prepare_cached("SELECT observation_member_anchor FROM events INDEXED BY events_observation_assistant_ledger_idx WHERE observation_assistant_ledger=?1 AND observation_assistant_ledger IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1").and_then(|mut statement| statement.query_row(params![position,self.cut.read.journal], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        if let Some(anchor) = anchor.flatten() {
            let sequence: u64 = self.connection.prepare_cached("SELECT sequence FROM events INDEXED BY events_observation_member_anchor_idx WHERE observation_member_anchor=?1 AND observation_members IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1").and_then(|mut statement| statement.query_row(params![anchor,self.cut.read.journal], |row| row.get(0))).map_err(|_| storage("native response member header unavailable"))?;
            return Ok(Some(ResponseIdentitySource::Attempt { sequence }));
        }
        let ordinal: Option<u64> = self.connection.prepare_cached("SELECT ordinal FROM inherited_response_provenance WHERE closing_ledger_position=?1").and_then(|mut statement| statement.query_row([position], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        Ok(ordinal.map(|ordinal| ResponseIdentitySource::Inherited { ordinal }))
    }

    /// Process membership and completed-response ownership have different
    /// acceptance rules. Share their local ledger lookup, retaining both rules.
    fn owner(
        &mut self,
        cursor: u64,
    ) -> Result<(Option<TurnProcessView>, Option<ResponseIdentitySource>), ConversationStoreError>
    {
        let position: Option<u64> = self.connection.prepare_cached("SELECT position FROM message_ledger INDEXED BY message_ledger_native_transcript_idx WHERE native_transcript=?1 AND native_transcript IS NOT NULL AND position<=?2").and_then(|mut statement| statement.query_row(params![cursor,self.cut.ledger_through], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        let Some(position) = position else {
            // Native control terminals and inherited terminal-only seats retain
            // their own validation; they are not ordinary assistant members.
            let process =
                super::observation_members::transcript_process(self.connection, self.cut, cursor)?;
            let sequence: Option<u64> = self.connection.prepare_cached("SELECT sequence FROM events INDEXED BY events_observation_transcript_idx WHERE observation_transcript=?1 AND observation_transcript IS NOT NULL AND sequence<=?2 AND observation_members IS NOT NULL").and_then(|mut statement| statement.query_row(params![cursor,self.cut.read.journal], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
            return Ok((
                process,
                sequence.map(|sequence| ResponseIdentitySource::Attempt { sequence }),
            ));
        };
        let role: Option<String> = self.connection.prepare_cached("SELECT json_extract(header_json,'$.role') FROM observation_headers INDEXED BY observation_headers_ledger_idx WHERE ledger_position=?1 AND ledger_position IS NOT NULL AND transcript_position=?2 AND journal_position<=?3").and_then(|mut statement| statement.query_row(params![position,cursor,self.cut.read.journal], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        let role = role.ok_or_else(|| storage("native member message header unavailable"))?;
        let proposal: Option<u64> = self.connection.prepare_cached("SELECT observation_ledger FROM canonical_tool_calls INDEXED BY canonical_tool_calls_result_ledger_idx WHERE result_ledger_position=?1 AND result_ledger_position IS NOT NULL").and_then(|mut statement| statement.query_row([position], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        if role == "tool" && proposal.is_none() {
            return Err(storage("native Tool member owner unavailable"));
        }
        let source_position = proposal.unwrap_or(position);
        let process_position = if role == "tool" {
            source_position
        } else {
            position
        };
        let source = self.source_at_position(source_position)?;
        if role != "assistant" && role != "tool" {
            return Ok((None, source));
        }
        let inherited: Option<Option<u64>> = self.connection.prepare_cached("SELECT turn_ordinal FROM inherited_message_membership WHERE assistant_ledger_position=?1").and_then(|mut statement| statement.query_row([process_position], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        if cursor <= self.cut.inherited_through.get() && inherited.is_none() {
            return Err(storage("inherited native member relationship absent"));
        }
        if inherited.is_some() {
            // Includes relationship identity, original turn-header and member
            // checks. An inherited process is not inferred from response data.
            return Ok((
                super::observation_members::transcript_process(self.connection, self.cut, cursor)?,
                source,
            ));
        }
        // Response aliases apply to every role, whereas only Tool process rows
        // follow their proposal. Preserve that distinction even for corrupt aliases.
        let process_source = if process_position == source_position {
            source.clone()
        } else {
            self.source_at_position(process_position)?
        };
        let Some(ResponseIdentitySource::Attempt { sequence }) = process_source.as_ref() else {
            return Ok((None, source));
        };
        let process = if let Some(process) = self.processes.get(sequence) {
            process.clone()
        } else {
            let counts = self.members(*sequence)?;
            let process =
                super::observation_members::process_view(self.connection, self.cut, counts, None)?;
            self.processes.insert(*sequence, process.clone());
            process
        };
        Ok((process, source))
    }

    fn append_revision(
        &self,
        position: u64,
    ) -> Result<Option<SurfaceRevision>, ConversationStoreError> {
        // Retain the canonical-locator check performed by the individual API.
        let position = super::content::canonical_position(
            self.connection,
            &CanonicalContentLocator::ByLedgerPosition { position },
        )?;
        let revision: Option<u64> = self
            .connection
            .prepare_cached("SELECT first_append_revision FROM message_ledger WHERE position=?1")
            .and_then(|mut statement| statement.query_row([position], |row| row.get(0)))
            .map_err(|error| storage(error.to_string()))?;
        Ok(revision.map(SurfaceRevision::new))
    }

    fn retry(
        &self,
        request: Option<u64>,
    ) -> Result<(Option<CanonicalContentLocator>, Option<MessageId>), ConversationStoreError> {
        let Some(request) = request else {
            return Ok((None, None));
        };
        super::observation_request::validate_start(
            self.connection,
            request,
            self.cut.read.journal,
        )?;
        let position: Option<u64> = self.connection.prepare_cached("SELECT retry_ledger_position FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1").and_then(|mut statement| statement.query_row([request], |row| row.get(0))).map_err(|error| storage(error.to_string()))?;
        let Some(position) = position else {
            return Ok((None, None));
        };
        let bytes: Vec<u8> = self.connection.prepare_cached("SELECT header_json FROM observation_headers INDEXED BY observation_headers_ledger_idx WHERE ledger_position=?1 AND reference_kind='message'").and_then(|mut statement| statement.query_row([position], |row| row.get(0))).map_err(|_| storage("native retry message header unavailable"))?;
        let header: crate::durable::observation::MessageHeader = serde_json::from_slice(&bytes)
            .map_err(|_| storage("native retry message header corrupt"))?;
        Ok((
            Some(CanonicalContentLocator::ByLedgerPosition { position }),
            header.identity,
        ))
    }

    fn response(
        &mut self,
        source: &ResponseIdentitySource,
    ) -> Result<Option<CompletedResponseView>, ConversationStoreError> {
        let key = source_key(source);
        if let Some(response) = self.responses.get(&key) {
            return Ok(response.clone());
        }
        let response = self.read_response(source)?;
        self.responses.insert(key, response.clone());
        Ok(response)
    }

    fn read_response(
        &mut self,
        source: &ResponseIdentitySource,
    ) -> Result<Option<CompletedResponseView>, ConversationStoreError> {
        match *source {
            ResponseIdentitySource::Inherited { ordinal } => {
                let Some(header) = self.inherited(ordinal)? else {
                    return Ok(None);
                };
                let revision = self
                    .append_revision(header.response.closing_ledger_position)?
                    .ok_or_else(|| {
                        ConversationStoreError::InvalidReference(
                            "inherited closing Surface append unavailable".into(),
                        )
                    })?;
                Ok(Some(
                    crate::runtime_client::response::inherited_response_view_at(
                        header.response,
                        revision,
                    ),
                ))
            }
            ResponseIdentitySource::Attempt { sequence } => {
                let summary = self.execution(sequence)?;
                let owner = self
                    .members(sequence)?
                    .process
                    .ok_or_else(|| storage("native response owner unavailable"))?;
                if summary.terminal_kind.as_deref() != Some("attempt_completed")
                    || !matches!(
                        summary.finish_reason,
                        Some(
                            crate::model::ModelFinishReason::Stop
                                | crate::model::ModelFinishReason::Refusal
                        )
                    )
                {
                    return Ok(None);
                }
                let Some(position) = summary.closing_ledger_position else {
                    return Ok(None);
                };
                let Some(revision) = self.append_revision(position)? else {
                    return Ok(None);
                };
                let Some(ended) = summary.ended_at else {
                    return Ok(None);
                };
                let (retry_locator, retry_id) = self.retry(summary.last_request_sequence)?;
                Ok(Some(
                    crate::runtime_client::response::completed_response_view(
                        self.conversation,
                        summary,
                        owner,
                        revision,
                        retry_locator,
                        retry_id,
                        ended,
                    ),
                ))
            }
        }
    }

    fn models(
        &mut self,
        source: &ResponseIdentitySource,
    ) -> Result<Models, ConversationStoreError> {
        match *source {
            ResponseIdentitySource::Attempt { sequence } => {
                let summary = self.execution(sequence)?;
                Ok((
                    summary.models_total,
                    summary.models.len() as u64,
                    summary.models_next_offset,
                ))
            }
            ResponseIdentitySource::Inherited { ordinal } => {
                let header = self
                    .inherited(ordinal)?
                    .ok_or_else(|| storage("native response header unavailable"))?;
                Ok((
                    header.models_total,
                    header.response.models.len() as u64,
                    header.models_next_offset,
                ))
            }
        }
    }

    fn identity_fields(
        &mut self,
        source: &ResponseIdentitySource,
    ) -> Result<Vec<ResponseIdentityField>, ConversationStoreError> {
        let key = source_key(source);
        if let Some(fields) = self.fields.get(&key) {
            return Ok(fields.clone());
        }
        let fields = super::response_identity_fields(self.connection, self.cut, source)?;
        self.fields.insert(key, fields.clone());
        Ok(fields)
    }

    fn item(
        &mut self,
        selection: &ResponsePageSelection,
    ) -> Result<ResponsePageItem, ConversationStoreError> {
        let (process, source) = self.owner(selection.cursor)?;
        let deferred = matches!(&selection.kind, ResponsePageSelectionKind::Deferred(_));
        let mut failure_deferred = false;
        if !deferred && let Some(ResponseIdentitySource::Attempt { sequence }) = source.as_ref() {
            let summary = self.execution(*sequence)?;
            failure_deferred = summary.failure_deferred;
            if failure_deferred && summary.failure_readable_end == 0 {
                return Err(storage("native failure layout unavailable"));
            }
        }
        let mut response = if matches!(&selection.kind, ResponsePageSelectionKind::Process) {
            None
        } else {
            source
                .as_ref()
                .map(|source| self.response(source))
                .transpose()?
                .flatten()
        };
        if let Some(value) = &response {
            let matches = match &selection.kind {
                ResponsePageSelectionKind::Process => false,
                ResponsePageSelectionKind::Message(id) => {
                    let position: u64 = self
                        .connection
                        .prepare_cached("SELECT position FROM message_ledger WHERE message_id=?1")
                        .and_then(|mut statement| {
                            statement.query_row([id.as_str()], |row| row.get(0))
                        })
                        .map_err(|_| storage("native response locator unavailable"))?;
                    value.closing_locator == CanonicalContentLocator::ByLedgerPosition { position }
                }
                ResponsePageSelectionKind::Deferred(locator) => {
                    locator.as_ref() == serde_json::to_value(&value.closing_locator).ok().as_ref()
                }
            };
            if !matches {
                response = None;
            }
        }
        let visible = response.is_some() || (deferred && process.is_some());
        let models = if visible {
            source
                .as_ref()
                .map(|source| self.models(source))
                .transpose()?
        } else {
            None
        };
        let needs_fields = visible
            && (deferred
                || response
                    .as_ref()
                    .is_some_and(|value| !value.deferred_fields.is_empty()));
        let identity_fields = if needs_fields {
            source
                .as_ref()
                .map(|source| self.identity_fields(source))
                .transpose()?
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(ResponsePageItem {
            process,
            response,
            failure_deferred,
            models,
            identity_fields,
        })
    }
}

#[cfg(test)]
thread_local! {
    static READ_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}
#[cfg(test)]
pub(super) fn read_work() -> (usize, usize) {
    READ_WORK.with(|work| work.replace((0, 0)))
}
