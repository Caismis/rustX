//! Bounded response decoration with native proof reuse for completed owners.
use super::{Connection, ConversationStoreError, OptionalExtension, Transaction, params, storage};
use crate::conversation::SurfaceRevision;
use crate::durable::observation::{
    AttemptExecutionSummary, AttemptMemberCounts, HEADER_PAGE_MAX, InheritedResponseHeader,
    ObservationReadCut, ResponseIdentityField, ResponseIdentitySource, ResponsePage,
    ResponsePageItem, ResponsePageSelection, ResponsePageSelectionKind,
};
use crate::runtime::identity::{ConversationId, MessageId};
use crate::runtime_client::content::CanonicalContentLocator;
use crate::runtime_client::response::{CompletedResponseView, TurnProcessView};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

mod cache;
pub(super) use cache::Owner;

type Models = (u64, u64, Option<u64>);

struct Reader<'a> {
    connection: &'a Connection,
    conversation: &'a ConversationId,
    cut: &'a ObservationReadCut,
    executions: BTreeMap<u64, (AttemptExecutionSummary, u64, u64)>,
    cache: &'a Owner,
    snapshot: Option<cache::Snapshot>,
    dependencies: Cell<cache::Dependencies>,
    completed_executions: bool,
    fully_proven: bool,
    native_user_without_response: bool,
    hole_free_from: Option<u64>,
    members: BTreeMap<u64, AttemptMemberCounts>,
    processes: BTreeMap<u64, Option<TurnProcessView>>,
    inherited: BTreeMap<u64, Option<InheritedResponseHeader>>,
    responses: BTreeMap<(bool, u64), Option<CompletedResponseView>>,
    fields: BTreeMap<(bool, u64), Vec<ResponseIdentityField>>,
}

pub(super) fn read(
    transaction: &Transaction<'_>,
    owner: &Owner,
    conversation: &ConversationId,
    cut: &ObservationReadCut,
    selections: &[ResponsePageSelection],
) -> Result<ResponsePage, ConversationStoreError> {
    if selections.len() > HEADER_PAGE_MAX {
        return Err(ConversationStoreError::InvalidReference(
            "response page exceeds native header limit".into(),
        ));
    }
    let validated = super::observation::ValidatedCut::new(transaction, conversation, cut)?;
    Ok(read_validated(&validated, owner, selections)?.page)
}

pub(super) struct Read {
    pub page: ResponsePage,
    pub owners: Vec<Arc<ResponsePageItem>>,
}

/// Only a transaction-bound native proof can authorize skipping the cut query.
pub(super) fn read_validated(
    validated: &super::observation::ValidatedCut<'_, '_>,
    owner: &Owner,
    selections: &[ResponsePageSelection],
) -> Result<Read, ConversationStoreError> {
    let connection = validated.connection()?;
    let cut = validated.expected();
    let conversation = &cut.read.conversation_id;
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
        let missing: bool = connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_owner_holes_idx WHERE observation_owner_holes>0 AND observation_owner_holes IN (4,5,6,7) AND sequence<=?1)").and_then(|mut statement| statement.query_row([cut.read.journal], |row| row.get(0))).map_err(|error| storage(error.to_string()))?;
        if missing {
            return Err(storage("native response owner header unavailable"));
        }
    }
    let mut reader = Reader {
        connection,
        conversation,
        cut,
        cache: owner,
        snapshot: if selections.is_empty() {
            None
        } else {
            owner.capture(validated)?
        },
        dependencies: Cell::new(cache::Dependencies::default()),
        completed_executions: true,
        fully_proven: true,
        native_user_without_response: false,
        hole_free_from: None,
        executions: BTreeMap::new(),
        members: BTreeMap::new(),
        processes: BTreeMap::new(),
        inherited: BTreeMap::new(),
        responses: BTreeMap::new(),
        fields: BTreeMap::new(),
    };
    let owners = selections
        .iter()
        .map(|selection| reader.item(selection))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Read {
        page: ResponsePage {
            items: owners.iter().map(|item| item.as_ref().clone()).collect(),
            statistics: super::observation_statistics::read_validated(validated)?,
        },
        owners,
    })
}

fn source_key(source: &ResponseIdentitySource) -> (bool, u64) {
    match source {
        ResponseIdentitySource::Attempt { sequence } => (false, *sequence),
        ResponseIdentitySource::Inherited { ordinal } => (true, *ordinal),
    }
}

impl Reader<'_> {
    fn journal(&self, sequence: u64) {
        let mut deps = self.dependencies.get();
        deps.journal = deps.journal.max(sequence);
        self.dependencies.set(deps);
    }
    fn ledger(&self, position: u64) {
        let mut deps = self.dependencies.get();
        deps.ledger = deps.ledger.max(position);
        self.dependencies.set(deps);
    }
    fn execution_proof(&mut self, summary: &AttemptExecutionSummary, anchor: u64, latest: u64) {
        self.journal(anchor.max(latest));
        for sequence in [
            summary.last_request_sequence,
            summary.closing_identity_sequence,
            summary.terminal_sequence,
            summary.finish_reason_sequence,
        ]
        .into_iter()
        .flatten()
        {
            self.journal(sequence);
        }
        if let Some(position) = summary.closing_ledger_position {
            self.ledger(position);
        }
        let mut deps = self.dependencies.get();
        deps.execution_from = Some(
            deps.execution_from
                .map_or(anchor, |prior| prior.min(anchor)),
        );
        self.dependencies.set(deps);
        self.completed_executions &= summary.terminal_kind.is_some();
    }
    fn execution(
        &mut self,
        sequence: u64,
    ) -> Result<AttemptExecutionSummary, ConversationStoreError> {
        self.journal(sequence);
        if let Some((summary, anchor, latest)) = self.executions.get(&sequence).cloned() {
            self.execution_proof(&summary, anchor, latest);
            return Ok(summary);
        }
        #[cfg(test)]
        READ_WORK.with(|work| {
            let (executions, members) = work.get();
            work.set((executions + 1, members));
        });
        let (summary, anchor, latest) = super::observation_execution::summary_selected_with_proof(
            self.connection,
            sequence,
            self.cut.read.journal,
        )?;
        self.execution_proof(&summary, anchor, latest);
        self.executions
            .insert(sequence, (summary.clone(), anchor, latest));
        Ok(summary)
    }

    fn members(&mut self, sequence: u64) -> Result<AttemptMemberCounts, ConversationStoreError> {
        self.journal(sequence);
        if let Some(counts) = self.members.get(&sequence) {
            return Ok(counts.clone());
        }
        #[cfg(test)]
        READ_WORK.with(|work| {
            let (executions, members) = work.get();
            work.set((executions, members + 1));
        });
        let counts = super::observation_members::at_sequence(self.connection, sequence)?;
        if let Some(process) = &counts.process {
            self.journal(
                process
                    .owner_anchor
                    .max(process.native_start.unwrap_or(0))
                    .max(process.native_terminal.unwrap_or(0)),
            );
        }
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
        self.ledger(position);
        let anchor: Option<Option<u64>> = self.connection.prepare_cached("SELECT observation_member_anchor FROM events INDEXED BY events_observation_assistant_ledger_idx WHERE observation_assistant_ledger=?1 AND observation_assistant_ledger IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1").and_then(|mut statement| statement.query_row(params![position,self.cut.read.journal], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        if let Some(anchor) = anchor.flatten() {
            self.journal(anchor);
            let sequence: u64 = self.connection.prepare_cached("SELECT sequence FROM events INDEXED BY events_observation_member_anchor_idx WHERE observation_member_anchor=?1 AND observation_members IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1").and_then(|mut statement| statement.query_row(params![anchor,self.cut.read.journal], |row| row.get(0))).map_err(|_| storage("native response member header unavailable"))?;
            self.journal(sequence);
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
        #[cfg(test)]
        CACHE_WORK.with(|work| {
            let (hits, reads, holes) = work.get();
            work.set((hits, reads + 1, holes));
        });
        let mut deps = self.dependencies.get();
        deps.transcript = deps.transcript.max(cursor);
        self.dependencies.set(deps);
        let position: Option<u64> = self.connection.prepare_cached("SELECT position FROM message_ledger INDEXED BY message_ledger_native_transcript_idx WHERE native_transcript=?1 AND native_transcript IS NOT NULL AND position<=?2").and_then(|mut statement| statement.query_row(params![cursor,self.cut.ledger_through], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        let Some(position) = position else {
            // This delegated reader does not return its native-terminal
            // dependencies. Preserve its full validation without certifying it.
            self.fully_proven = false;
            // Native control terminals and inherited terminal-only seats retain
            // their own validation; they are not ordinary assistant members.
            let process =
                super::observation_members::transcript_process(self.connection, self.cut, cursor)?;
            let sequence: Option<u64> = self.connection.prepare_cached("SELECT sequence FROM events INDEXED BY events_observation_transcript_idx WHERE observation_transcript=?1 AND observation_transcript IS NOT NULL AND sequence<=?2 AND observation_members IS NOT NULL").and_then(|mut statement| statement.query_row(params![cursor,self.cut.read.journal], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
            if let Some(sequence) = sequence {
                self.journal(sequence);
            }
            return Ok((
                process,
                sequence.map(|sequence| ResponseIdentitySource::Attempt { sequence }),
            ));
        };
        self.ledger(position);
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
            self.native_user_without_response = role == "user" && source.is_none();
            return Ok((None, source));
        }
        let inherited: Option<Option<u64>> = self.connection.prepare_cached("SELECT turn_ordinal FROM inherited_message_membership WHERE assistant_ledger_position=?1").and_then(|mut statement| statement.query_row([process_position], |row| row.get(0))).optional().map_err(|error| storage(error.to_string()))?;
        if cursor <= self.cut.inherited_through.get() && inherited.is_none() {
            return Err(storage("inherited native member relationship absent"));
        }
        if inherited.is_some() {
            self.fully_proven = false;
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
        self.ledger(position);
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
        self.journal(request);
        super::observation_request::validate_start(
            self.connection,
            request,
            self.cut.read.journal,
        )?;
        let position: Option<u64> = self.connection.prepare_cached("SELECT retry_ledger_position FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1").and_then(|mut statement| statement.query_row([request], |row| row.get(0))).map_err(|error| storage(error.to_string()))?;
        let Some(position) = position else {
            return Ok((None, None));
        };
        self.ledger(position);
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
        if let Some(response) = self.responses.get(&key).cloned() {
            if let ResponseIdentitySource::Attempt { sequence } = source {
                self.execution(*sequence)?;
            }
            return Ok(response);
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
        // Identity field lookup also reads the original selected execution row,
        // whose identity references can differ from the latest owner summary.
        self.journal(self.cut.read.journal);
        let fields = super::response_identity_fields(self.connection, self.cut, source)?;
        self.fields.insert(key, fields.clone());
        Ok(fields)
    }

    fn item(
        &mut self,
        selection: &ResponsePageSelection,
    ) -> Result<Arc<ResponsePageItem>, ConversationStoreError> {
        if let Some(snapshot) = self.snapshot
            && let Some(hit) = self.cache.get(snapshot, self.cut, selection)?
        {
            if let Some(from) = hit.execution_from {
                self.check_execution_holes(from)?;
            }
            #[cfg(test)]
            CACHE_WORK.with(|work| {
                let (hits, reads, holes) = work.get();
                work.set((hits + 1, reads, holes));
            });
            return Ok(hit.item);
        }
        // Frontiers accumulate across this bounded page, so local owner maps
        // carry all prior native dependencies without repeating their SQL. The
        // execution-hole interval is item-specific: a previous owner must not
        // make a later owner fail for an unrelated earlier hole.
        let mut deps = self.dependencies.get();
        deps.execution_from = None;
        self.dependencies.set(deps);
        self.completed_executions = true;
        self.fully_proven = true;
        self.native_user_without_response = false;
        let item = Arc::new(self.read_item(selection)?);
        let completed = item.process.as_ref().is_some_and(|process| {
            process.outcome != crate::runtime_client::response::TurnProcessOutcome::Running
        });
        if self.fully_proven
            && self.completed_executions
            && (completed || self.native_user_without_response)
            && let Some(snapshot) = self.snapshot
        {
            self.cache.insert(
                snapshot,
                self.cut,
                selection,
                &item,
                self.dependencies.get(),
            )?;
        }
        Ok(item)
    }
    fn check_execution_holes(&mut self, from: u64) -> Result<(), ConversationStoreError> {
        if self.hole_free_from.is_some_and(|prior| prior <= from) {
            return Ok(());
        }
        #[cfg(test)]
        CACHE_WORK.with(|work| {
            let (hits, reads, holes) = work.get();
            work.set((hits, reads, holes + 1));
        });
        let missing: bool = self.connection.prepare_cached("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_owner_holes_idx WHERE observation_owner_holes>0 AND observation_owner_holes IN (1,3,5,7) AND sequence>=?1 AND sequence<=?2)")
            .and_then(|mut statement| statement.query_row(params![from,self.cut.read.journal], |row| row.get(0))).map_err(|error| storage(error.to_string()))?;
        if missing {
            return Err(storage("native execution header evidence unavailable"));
        }
        self.hole_free_from = Some(from);
        Ok(())
    }

    fn read_item(
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
                    self.ledger(position);
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

#[cfg(test)]
thread_local! {
    static CACHE_WORK: std::cell::Cell<(usize, usize, usize)> = const { std::cell::Cell::new((0, 0, 0)) };
}
/// Cache hits, original native-owner queries, and retained current-prefix hole queries.
#[cfg(test)]
pub(super) fn cache_work() -> (usize, usize, usize) {
    CACHE_WORK.with(|work| work.replace((0, 0, 0)))
}
