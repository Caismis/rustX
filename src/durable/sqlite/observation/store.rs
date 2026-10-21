//! The `ObservationStore` implementation: bounded native observation reads.

use crate::durable::sqlite::canonical::ordinary_user_by_id;
use crate::durable::sqlite::canonical::transcript::TRANSCRIPT_ATTEMPTS_SQL;
use crate::durable::sqlite::{
    SqliteConversationStore, content, observation, observation_delivery, observation_event,
    observation_execution, observation_identity, observation_members, observation_occupancy,
    observation_projection, observation_readiness, observation_request, observation_state,
    observation_statistics, observation_trace, observation_turn, provenance, reading, seq_to_i64,
    storage,
};
use crate::durable::{ConversationStoreError, ObservationStore, TranscriptCursor};
use crate::runtime::identity::{AttemptId, MessageId, RequestId};
use rusqlite::OptionalExtension;
use rusqlite::{Connection, TransactionBehavior, params};
use std::sync::Arc;

impl ObservationStore for SqliteConversationStore {
    fn observation_trace_facts(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        query: &crate::durable::observation::TraceFactQuery,
    ) -> Result<Vec<crate::durable::observation::TraceFactHeader>, ConversationStoreError> {
        self.read_at_cut(cut, |transaction| {
            observation_trace::facts(transaction, cut, query)
        })
    }
    fn observation_request_accepted_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
    ) -> Result<Option<crate::observation::content::CanonicalContentLocator>, ConversationStoreError>
    {
        self.read_at_cut(cut, |transaction| {
            observation_request::validate_start(transaction, source_sequence, cut.read.journal)?;
            let position:Option<u64>=transaction.query_row("SELECT observation_accepted_ledger FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.flatten();
            Ok(position
                .filter(|position| *position <= cut.ledger_through)
                .map(|position| {
                    crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position }
                }))
        })
    }
    fn observation_request_summary_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::durable::observation::RequestReadSummary>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_trace::summary(&connection, source_sequence, through)
    }
    fn observation_request_predecessor_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::TraceRequestPredecessorHeader>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_trace::predecessor(&connection, source_sequence, through)
    }
    fn observation_request_retry_failure_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::model::error::ModelErrorKind>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_trace::retry_failure(&connection, source_sequence, through)
    }
    fn observation_trace_canonical_summary(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        position: u64,
    ) -> Result<crate::durable::observation::TraceCanonicalSummary, ConversationStoreError> {
        self.read_at_cut(cut, |transaction| {
            observation_trace::canonical(transaction, cut, position)
        })
    }
    fn observation_trace_tool_result(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<crate::durable::observation::TraceCanonicalSummary>, ConversationStoreError>
    {
        self.read_at_cut(cut, |transaction| {
            observation_trace::tool_result(transaction, cut, occurrence)
        })
    }
    fn observation_trace_tool_proposal(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<crate::durable::observation::TraceToolProposalSummary, ConversationStoreError> {
        self.read_at_cut(cut, |transaction| {
            observation_trace::proposal(transaction, cut, occurrence)
        })
    }
    fn observation_trace_tool_definition(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<crate::observation::trace::TraceFieldSource>, ConversationStoreError> {
        self.read_at_cut(cut, |transaction| {
            observation_trace::tool_definition(transaction, cut, occurrence)
        })
    }
    fn observation_trace_adoption(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::TraceAdoptionPage, ConversationStoreError> {
        self.read_at_cut(cut, |transaction| {
            observation_trace::adoption_page(transaction, cut, source_sequence, offset, limit)
        })
    }
    fn observation_request_context_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::TraceRequestContextPage, ConversationStoreError> {
        self.read_at_cut(cut, |transaction| {
            observation_trace::context(transaction, cut, source_sequence, offset, limit)
        })
    }
    fn request_summary_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<crate::durable::observation::RequestObservationHeader, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::header_at(&connection, source_sequence, through)
    }
    fn request_field_header_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::field_header_at(&connection, source_sequence, through, path)
    }
    fn request_field_page_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::page_at(&connection, source_sequence, through, path, offset, limit)
    }
    fn request_value_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::read_at(
            &connection,
            source_sequence,
            through,
            path,
            offset,
            max_bytes,
        )
    }
    fn request_outcome_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::RequestObservationOutcome>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_request::outcome_at(&connection, source_sequence, through)
    }
    fn request_outcome_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::outcome_read_at(
            &connection,
            source_sequence,
            through,
            path,
            offset,
            max_bytes,
        )
    }
    fn request_previous_identity(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError> {
        let connection = self.lock()?;
        let previous:Option<String>=connection.query_row("SELECT previous_request_id FROM request_snapshots WHERE request_id=?1 AND started_sequence<=?2",params![request.as_str(),through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?.ok_or_else(||ConversationStoreError::InvalidReference("request outside native read cut".into()))?;
        Ok(previous.map(RequestId::new))
    }
    fn request_outcome_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::RequestObservationOutcome>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_request::outcome(&connection, request, through)
    }
    fn request_outcome_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::outcome_read(&connection, request, through, path, offset, max_bytes)
    }
    fn request_field_header(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::field_header(&connection, request, through, path)
    }
    fn request_identity_at(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError> {
        let connection = self.lock()?;
        let request:Option<String>=connection.query_row("SELECT request_id FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1 AND started_sequence<=?2",params![sequence,through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        Ok(request.map(RequestId::new))
    }
    fn request_summary_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<crate::durable::observation::RequestObservationHeader, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::header(&connection, request, through)
    }
    fn request_field_page(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::page(&connection, request, through, path, offset, limit)
    }
    fn request_value_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::read(&connection, request, through, path, offset, max_bytes)
    }
    fn validate_content_source_cut(
        &self,
        reference: &crate::observation::content::ContentRef,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<(), ConversationStoreError> {
        let connection = self.lock()?;
        content::validate_source_cut(&connection, &self.conversation_id, reference, cut)
    }
    fn observation_inherited_attempt_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        kind: crate::observation::turn::InheritedAttemptIdentityKind,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        observation_identity::read(&connection, cut, kind, ordinal, offset, max_bytes)
    }
    #[cfg(test)]
    fn observation_inherited_native_oracle(
        &self,
        messages: &[MessageId],
    ) -> Result<crate::durable::observation::InheritedObservation, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::observation_native_oracle(&connection, messages)
    }
    fn observation_inherited_models(
        &self,
        response_ordinal: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::InheritedModelPage, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::observation_models(&connection, response_ordinal, offset, limit)
    }
    fn observation_inherited_model_read(
        &self,
        response_ordinal: u64,
        model_ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        provenance::observation_model_read(
            &connection,
            response_ordinal,
            model_ordinal,
            offset,
            max_bytes,
        )
    }
    fn observation_failure_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let chunk = observation_execution::failure_value(
            &transaction,
            attempt,
            through,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(chunk)
    }
    fn response_identity_source(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript_cursor: u64,
    ) -> Result<Option<crate::durable::observation::ResponseIdentitySource>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if transcript_cursor > cut.read.transcript {
            return Err(ConversationStoreError::InvalidReference(
                "response source beyond native transcript cut".into(),
            ));
        }
        let missing:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM events INDEXED BY events_observation_member_holes_sequence_idx WHERE observation_members_required=1 AND observation_members IS NULL AND sequence<=?1)",[cut.read.journal],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        if missing {
            return Err(storage("native response owner header unavailable"));
        }
        let position:Option<u64>=connection.query_row("SELECT position FROM message_ledger INDEXED BY message_ledger_native_transcript_idx WHERE native_transcript=?1 AND native_transcript IS NOT NULL AND position<=?2",params![transcript_cursor,cut.ledger_through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        let Some(mut position) = position else {
            let sequence:Option<u64>=connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_transcript_idx WHERE observation_transcript=?1 AND observation_transcript IS NOT NULL AND sequence<=?2 AND observation_members IS NOT NULL",params![transcript_cursor,cut.read.journal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
            return Ok(sequence.map(|sequence| {
                crate::durable::observation::ResponseIdentitySource::Attempt { sequence }
            }));
        };
        let proposal:Option<u64>=connection.query_row("SELECT observation_ledger FROM canonical_tool_calls INDEXED BY canonical_tool_calls_result_ledger_idx WHERE result_ledger_position=?1 AND result_ledger_position IS NOT NULL",[position],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        if let Some(proposal) = proposal {
            position = proposal;
        }
        let anchor: Option<Option<u64>> = connection.query_row("SELECT observation_member_anchor FROM events INDEXED BY events_observation_assistant_ledger_idx WHERE observation_assistant_ledger=?1 AND observation_assistant_ledger IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![position,cut.read.journal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        if let Some(anchor) = anchor.flatten() {
            let sequence:u64=connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_member_anchor_idx WHERE observation_member_anchor=?1 AND observation_members IS NOT NULL AND sequence<=?2 ORDER BY sequence DESC LIMIT 1",params![anchor,cut.read.journal],|row|row.get(0)).map_err(|_|storage("native response member header unavailable"))?;
            return Ok(Some(
                crate::durable::observation::ResponseIdentitySource::Attempt { sequence },
            ));
        }
        let ordinal:Option<u64>=connection.query_row("SELECT ordinal FROM inherited_response_provenance WHERE closing_ledger_position=?1",[position],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        Ok(ordinal.map(
            |ordinal| crate::durable::observation::ResponseIdentitySource::Inherited { ordinal },
        ))
    }
    fn inherited_response_summary(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ordinal: u64,
    ) -> Result<Option<crate::durable::observation::InheritedResponseHeader>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        let bytes:Option<Vec<u8>>=connection.query_row("SELECT display_provenance FROM inherited_response_provenance WHERE ordinal=?1 AND closing_ledger_position<=?2",params![ordinal,cut.ledger_through],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
        bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string())))
            .transpose()
    }
    fn response_identity_fields(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
    ) -> Result<Vec<crate::durable::observation::ResponseIdentityField>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        response_identity_fields(&connection, cut, source)
    }
    fn response_identity_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        let (key, field) = response_identity_key(&connection, cut, source, path)?;
        content::utf8_field_read(
            &connection,
            "response_identity",
            &key,
            field,
            offset,
            max_bytes,
        )
    }
    fn observation_execution_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<crate::durable::observation::AttemptExecutionSummary, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "execution outside source cut".into(),
            ));
        }
        observation_execution::summary_selected(&connection, sequence, cut.read.journal)
    }
    fn observation_members_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "member outside source cut".into(),
            ));
        }
        observation_members::at_sequence(&connection, sequence)
    }
    fn observation_attempt_summary(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<crate::durable::observation::AttemptExecutionSummary, ConversationStoreError> {
        let connection = self.lock()?;
        observation_execution::summary(&connection, attempt, through)
    }
    fn todo_observation_head(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<crate::durable::NativeTodoObservation>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        observation_state::todo_head(&connection, 1, cut.ledger_through)
    }
    fn observation_state_source(
        &self,
        domain: &str,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<String>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::source(&connection, domain, cut)
    }
    fn observation_state_header(
        &self,
        domain: &str,
        source_id: &str,
        cut: &crate::durable::observation::ObservationReadCut,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::header(&connection, domain, source_id, cut, path)
    }
    fn observation_state_page(
        &self,
        domain: &str,
        source_id: &str,
        cut: &crate::durable::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::page(&connection, domain, source_id, cut, path, offset, limit)
    }
    fn observation_state_value_read(
        &self,
        domain: &str,
        source_id: &str,
        cut: &crate::durable::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_state::read(&connection, domain, source_id, cut, path, offset, max_bytes)
    }
    fn observation_models_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AttemptModelNamePage, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "native owner outside cut".into(),
            ));
        }
        observation_execution::models_at(&connection, sequence, cut.read.journal, offset, limit)
    }
    fn observation_model_read_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "native owner outside cut".into(),
            ));
        }
        observation_execution::model_value_at(
            &connection,
            sequence,
            cut.read.journal,
            ordinal,
            offset,
            max_bytes,
        )
    }
    fn observation_failure_read_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if sequence > cut.read.journal {
            return Err(ConversationStoreError::InvalidReference(
                "native owner outside cut".into(),
            ));
        }
        observation_execution::failure_at(&connection, sequence, offset, max_bytes)
    }
    fn observation_models_page(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AttemptModelNamePage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let page = observation_execution::models(&transaction, attempt, through, offset, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(page)
    }
    fn observation_model_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let chunk = observation_execution::model_value(
            &transaction,
            attempt,
            through,
            ordinal,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(chunk)
    }
    fn observation_request_retry_locator_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::observation::content::CanonicalContentLocator>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_request::validate_start(&connection, source_sequence, through)?;
        let position:Option<u64>=connection.query_row("SELECT retry_ledger_position FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        Ok(position.map(|position| {
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position }
        }))
    }
    fn observation_request_retry_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_request::validate_start(&connection, source_sequence, through)?;
        let position:Option<u64>=connection.query_row("SELECT retry_ledger_position FROM request_snapshots INDEXED BY request_snapshots_started_idx WHERE started_sequence=?1",[source_sequence],|row|row.get(0)).map_err(|error|storage(error.to_string()))?;
        let Some(position) = position else {
            return Ok(None);
        };
        let bytes:Vec<u8>=connection.query_row("SELECT header_json FROM observation_headers INDEXED BY observation_headers_ledger_idx WHERE ledger_position=?1 AND reference_kind='message'",[position],|row|row.get(0)).map_err(|_|storage("native retry message header unavailable"))?;
        let header: crate::durable::observation::MessageHeader = serde_json::from_slice(&bytes)
            .map_err(|_| storage("native retry message header corrupt"))?;
        Ok(header.identity)
    }
    fn observation_request_retry_locator(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<crate::observation::content::CanonicalContentLocator>, ConversationStoreError>
    {
        let connection = self.lock()?;
        let position: Option<u64> = connection
            .query_row(
                "SELECT retry_ledger_position FROM request_snapshots WHERE request_id=?1",
                [request_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(position.map(|position| {
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position }
        }))
    }
    fn observation_request_retry(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<MessageId>, ConversationStoreError> {
        let connection = self.lock()?;
        let id: Option<String> = connection
            .query_row(
                "SELECT CASE WHEN length(CAST(retry_message_id AS BLOB))<=256 THEN retry_message_id ELSE NULL END FROM request_snapshots WHERE request_id=?1",
                [request_id.as_str()],
                |row| row.get(0),
            )
            .map_err(|error| storage(error.to_string()))?;
        Ok(id.map(MessageId::new))
    }
    fn observation_member_counts(
        &self,
        message_id: &MessageId,
        through: u64,
    ) -> Result<Option<crate::durable::observation::AttemptMemberCounts>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_members::message_members(&connection, message_id, through)
    }
    fn observation_attempt_members(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError> {
        let connection = self.lock()?;
        observation_members::members(&connection, attempt, through)
    }
    fn observation_transcript_process(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript: u64,
    ) -> Result<Option<crate::observation::turn::TurnProcessView>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        observation_members::transcript_process(&connection, cut, transcript)
    }
    fn audit_block_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AuditBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation::audit_blocks(
            &connection,
            &self.conversation_id,
            cut,
            transcript,
            offset,
            limit,
        )
    }
    fn publication_settlement_head(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<
        Vec<crate::observation::transcript::RuntimeClientPublicationSettlementRecord>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation::publication_settlement_head(&connection, &self.conversation_id, cut)
    }
    fn observation_trace_total(&self, through: u64) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        observation_statistics::trace_total(&connection, through)
    }
    fn observation_occupancy(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
        ConversationStoreError,
    > {
        let connection = self.lock()?;
        observation_occupancy::read(&connection, through)
    }
    fn observation_occupancy_model_value_read(
        &self,
        sequence: Option<u64>,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_occupancy::model_read(&connection, sequence, through, offset, max_bytes)
    }
    fn observation_statistics(
        &self,
        through: u64,
    ) -> Result<crate::observation::statistics::ConversationStatistics, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_statistics::read(&connection, through)
    }
    fn observation_agent_statistics(
        &self,
        through: u64,
    ) -> Result<crate::observation::statistics::AgentStatistics, ConversationStoreError> {
        let connection = self.lock()?;
        observation_statistics::agent_read(&connection, through)
    }
    fn pending_header_page(
        &self,
        cut: &crate::durable::reading::ConversationReadCut,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<crate::durable::observation::PendingHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::pending_page(
            &transaction,
            &self.conversation_id,
            cut,
            after_sequence,
            limit,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn pending_value_read(
        &self,
        cut: &crate::durable::reading::ConversationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::pending_value(
            &transaction,
            &self.conversation_id,
            cut,
            sequence,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn read_window_headers(
        &self,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::durable::observation::ObservationWindow, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::window(&transaction, &self.conversation_id, at, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn event_transcript_cursor_at_sequence(
        &self,
        sequence: u64,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError> {
        let connection = self.lock()?;
        let cursor: Option<u64> = connection
            .query_row(
                "SELECT observation_transcript FROM events WHERE sequence=?1",
                [sequence],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?
            .flatten();
        Ok(cursor.map(TranscriptCursor::new))
    }
    fn observation_event_kind(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<String>, ConversationStoreError> {
        let connection = self.lock()?;
        observation_event::kind(&connection, sequence, through)
    }
    fn observation_event_witness(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<crate::durable::observation::ObservationEventWitness>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation_event::witness(&connection, sequence, through)
    }
    fn event_value_page(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation_event::page(&connection, sequence, through, path, offset, limit)
    }
    fn event_value_read(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        let connection = self.lock()?;
        observation_event::read(&connection, sequence, through, path, offset, max_bytes)
    }
    fn latest_observation_events(
        &self,
        through: u64,
        kinds: &[String],
        attempt: Option<&AttemptId>,
        limit: usize,
    ) -> Result<Vec<crate::durable::observation::ObservationEventHeader>, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::latest_events(&connection, through, kinds, attempt, limit)
    }
    fn tool_call_occurrence_at(
        &self,
        call_id: &crate::runtime::identity::ToolCallId,
        attempt: &AttemptId,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<crate::message::types::ToolCallOccurrenceRef>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::tool_occurrence(&connection, &self.conversation_id, call_id, attempt, cut)
    }
    fn tool_association_headers(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError>
    {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::tools(
            &transaction,
            &self.conversation_id,
            cut,
            message_id,
            offset,
            limit,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn tool_association_headers_by_locator(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError>
    {
        let connection = self.lock()?;
        observation::tools_by_locator(
            &connection,
            &self.conversation_id,
            cut,
            locator,
            offset,
            limit,
        )
    }
    fn ordinary_user_message(
        &self,
        locator: &crate::observation::content::CanonicalContentLocator,
    ) -> Result<bool, ConversationStoreError> {
        let connection = self.lock()?;
        match locator {
            crate::observation::content::CanonicalContentLocator::ByMessageId { message_id } => {
                ordinary_user_by_id(&connection, message_id)
            }
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position } => {
                connection
                    .query_row(
                        "SELECT ordinary_user FROM message_ledger WHERE position=?1",
                        [position],
                        |row| row.get(0),
                    )
                    .optional()
                    .map(|value| value.unwrap_or(false))
                    .map_err(|error| storage(error.to_string()))
            }
        }
    }
    fn message_ledger_position(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<u64>, ConversationStoreError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT position FROM message_ledger WHERE message_id=?1",
                [message_id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))
    }
    fn message_header(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<crate::durable::observation::MessageHeader>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::message(&connection, message_id)
    }
    fn message_block_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::blocks(
            &transaction,
            &self.conversation_id,
            cut,
            message_id,
            offset,
            limit,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn message_block_header_page_by_locator(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        observation::blocks_by_locator(
            &connection,
            &self.conversation_id,
            cut,
            locator,
            offset,
            limit,
        )
    }
    fn observation_tool_trace_sequence(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ledger_position: u64,
        block_index: crate::message::types::ContentBlockIndex,
    ) -> Result<Option<u64>, ConversationStoreError> {
        let connection = self.lock()?;
        observation::validate(&connection, &self.conversation_id, cut)?;
        if ledger_position == 0 || ledger_position > cut.ledger_through {
            return Err(ConversationStoreError::InvalidReference(
                "tool trace source beyond captured cut".into(),
            ));
        }
        connection.query_row("SELECT sequence FROM events INDEXED BY events_observation_tool_lifecycle_idx WHERE observation_tool_ledger=?1 AND observation_tool_block=?2 AND observation_tool_phase=1 AND sequence<=?3 ORDER BY sequence DESC LIMIT 1",params![ledger_position,block_index.get(),seq_to_i64(cut.read.journal)?],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))
    }
    fn observation_read_cut(
        &self,
    ) -> Result<crate::durable::observation::ObservationReadCut, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = observation::cut(&transaction, &self.conversation_id)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn message_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result =
            observation::messages(&transaction, &self.conversation_id, cut, offset, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn transcript_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<crate::durable::observation::TranscriptHeaderPage, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result =
            observation::transcript(&transaction, &self.conversation_id, cut, before, limit)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn transcript_attempts(
        &self,
        messages: &[MessageId],
        through: u64,
    ) -> Result<Vec<AttemptId>, ConversationStoreError> {
        let messages: std::collections::BTreeSet<_> = messages.iter().collect();
        let connection = self.lock()?;
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = connection
            .prepare_cached(TRANSCRIPT_ATTEMPTS_SQL)
            .map_err(|error| storage(error.to_string()))?;
        let mut attempts = std::collections::BTreeSet::new();
        for message in messages {
            let rows = statement
                .query_map(params![message.as_str(), seq_to_i64(through)?], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|error| storage(error.to_string()))?;
            for row in rows {
                attempts.insert(AttemptId::new(
                    row.map_err(|error| storage(error.to_string()))?,
                ));
            }
        }
        Ok(attempts.into_iter().collect())
    }
    fn conversation_read_cut(
        &self,
    ) -> Result<crate::durable::reading::ConversationReadCut, ConversationStoreError> {
        self.read_transaction(|transaction| reading::cut(transaction, &self.conversation_id))
    }
    fn observation_turns(
        &self,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::observation::turn::ObservedTurnPage, ConversationStoreError> {
        self.read_transaction(|transaction| {
            observation_turn::page(transaction, &self.conversation_id, offset, limit)
        })
    }
    fn observation_file_delivery(
        &self,
        locator: &crate::observation::content::CanonicalContentLocator,
        index: u64,
    ) -> Result<crate::durable::observation::NativeFileDelivery, ConversationStoreError> {
        self.read_transaction(|transaction| {
            observation_delivery::selected(transaction, &self.conversation_id, locator, index)
        })
    }
    fn observation_deliveries(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::observation::delivery::RuntimeClientDeliveryHead, ConversationStoreError>
    {
        content::validate_canonical_locator(locator)?;
        self.read_at_cut(cut, |transaction| {
            let position = content::canonical_position(transaction, locator)?;
            if position > cut.ledger_through {
                return Err(ConversationStoreError::InvalidReference(
                    "delivery beyond captured canonical cut".into(),
                ));
            }
            observation_delivery::head(transaction, position, offset, limit)
        })
    }
    fn observation_delivery_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        index: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        content::validate_canonical_locator(locator)?;
        self.read_at_cut(cut, |transaction| {
            let position = content::canonical_position(transaction, locator)?;
            if position > cut.ledger_through {
                return Err(ConversationStoreError::InvalidReference(
                    "delivery beyond captured canonical cut".into(),
                ));
            }
            observation_delivery::read(transaction, position, index, offset, max_bytes)
        })
    }
    fn conversation_turns(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<crate::durable::reading::ConversationTurnPage, ConversationStoreError> {
        self.read_transaction(|transaction| {
            reading::turns(transaction, &self.conversation_id, offset, limit)
        })
    }
    fn conversation_window(
        &self,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::durable::reading::DurableConversationWindow, ConversationStoreError> {
        self.read_transaction(|transaction| {
            reading::window(transaction, &self.conversation_id, at, limit)
        })
    }
    fn goal_observation_view(
        &self,
    ) -> Result<crate::observation::goal::RuntimeClientGoalView, ConversationStoreError> {
        let connection = self.lock()?;
        let bytes: Option<Option<Vec<u8>>> = connection
            .query_row(
                "SELECT observation_header FROM goal_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage(error.to_string()))?;
        match bytes {
            None => Ok(crate::observation::goal::RuntimeClientGoalView { current: None }),
            Some(None) => Err(storage("native Goal display header unavailable")),
            Some(Some(bytes)) => {
                serde_json::from_slice(&bytes).map_err(|error| storage(error.to_string()))
            }
        }
    }
    fn observation_projection_provenance_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        activation: u64,
        item: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
        self.read_transaction(|transaction| {
            observation_projection::provenance_read(
                transaction,
                &self.conversation_id,
                cut,
                activation,
                item,
                path,
                offset,
                max_bytes,
            )
        })
    }
    fn observation_projection_head(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::observation::head::RuntimeClientProjectionHeadView>,
        ConversationStoreError,
    > {
        self.read_transaction(|transaction| observation_projection::read(transaction, through))
    }
    fn observation_projection_activation(
        &self,
        activation: crate::context::checkpoint::ProjectionActivationId,
    ) -> Result<crate::observation::head::RuntimeClientProjectionHeadView, ConversationStoreError>
    {
        // One bounded SELECT validated in memory: no transaction whose
        // rollback would retire the committed append certificate.
        let connection = self.lock()?;
        observation_projection::read_id(&connection, activation.0)
    }
    fn mark_observation_ready(&self, first: u64, last: u64) -> Result<(), ConversationStoreError> {
        if first == 0 || first > last {
            return Err(ConversationStoreError::InvalidReference(
                "invalid observation readiness range".into(),
            ));
        }
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark(&transaction, first, last)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        // Never wake under the storage or observer-registration lock, and never
        // before the tiny derived layout transaction physically commits.
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(last);
        }
        Ok(())
    }
    fn mark_observation_direct(&self, first: u64, last: u64) -> Result<(), ConversationStoreError> {
        if first == 0 || first > last {
            return Err(ConversationStoreError::InvalidReference(
                "invalid observation readiness range".into(),
            ));
        }
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark_direct(&transaction, first, last)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        // Never wake under the storage or observer-registration lock, and never
        // before the tiny derived layout transaction physically commits.
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(last);
        }
        Ok(())
    }
    fn observation_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        observation_readiness::frontier(&connection, after, through)
    }
    fn mark_observation_transcript_ready(&self, cursor: u64) -> Result<(), ConversationStoreError> {
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark_transcript(&transaction, cursor)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(0);
        }
        Ok(())
    }
    fn mark_canonical_transcript_ready(
        &self,
        first_ledger: u64,
        last_ledger: u64,
    ) -> Result<(), ConversationStoreError> {
        {
            let mut connection = self.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| storage(error.to_string()))?;
            observation_readiness::mark_canonical(&transaction, first_ledger, last_ledger)?;
            transaction
                .commit()
                .map_err(|error| storage(error.to_string()))?;
        }
        let observer = self
            .journal_observer
            .lock()
            .expect("Journal observer lock")
            .as_ref()
            .map(|(observer, _)| Arc::clone(observer));
        if let Some(observer) = observer {
            observer.ready(0);
        }
        Ok(())
    }
    fn observation_transcript_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError> {
        let connection = self.lock()?;
        observation_readiness::transcript_frontier(&connection, after, through)
    }
    fn transcript_source_ready(
        &self,
    ) -> Result<crate::durable::observation::TranscriptSourceReady, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let read_cut = observation::cut(&transaction, &self.conversation_id)?;
        let prefix =
            observation_readiness::transcript_frontier(&transaction, 0, read_cut.read.transcript)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(crate::durable::observation::TranscriptSourceReady { prefix, read_cut })
    }
    fn live_block_header_page(
        &self,
        source: &crate::observation::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::observation::content::LiveBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        content::live_block_header_page(
            &connection,
            &self.conversation_id,
            source,
            through_revision,
            offset,
            limit,
        )
    }
    fn live_tool_header_page(
        &self,
        source: &crate::observation::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::observation::content::LiveBlockHeaderPage, ConversationStoreError> {
        let connection = self.lock()?;
        content::live_tool_header_page(
            &connection,
            &self.conversation_id,
            source,
            through_revision,
            offset,
            limit,
        )
    }
    fn content_descriptor_at(
        &self,
        source: &crate::observation::content::ContentSource,
        representation: crate::observation::content::ContentRepresentation,
        revision: u64,
    ) -> Result<Option<crate::observation::content::ContentRef>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = content::descriptor_at(
            &transaction,
            &self.conversation_id,
            source,
            representation,
            revision,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn content_descriptor(
        &self,
        source: &crate::observation::content::ContentSource,
        representation: crate::observation::content::ContentRepresentation,
    ) -> Result<Option<crate::observation::content::ContentRef>, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result =
            content::descriptor(&transaction, &self.conversation_id, source, representation)?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
    fn content_read(
        &self,
        reference: &crate::observation::content::ContentRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::observation::content::ContentChunk, ConversationStoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| storage(error.to_string()))?;
        let result = content::read(
            &transaction,
            &self.conversation_id,
            reference,
            offset,
            max_bytes,
        )?;
        transaction
            .commit()
            .map_err(|error| storage(error.to_string()))?;
        Ok(result)
    }
}

pub(in crate::durable::sqlite) fn response_identity_key(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    source: &crate::durable::observation::ResponseIdentitySource,
    path: &[String],
) -> Result<(String, u32), ConversationStoreError> {
    let field = match path {
        [field] if field == "closing_message_id" => 0,
        [field] if field == "retry_message_id" => 1,
        [origin, field] if origin == "origin" && field == "closing_message_id" => 2,
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "unknown response identity source field".into(),
            ));
        }
    };
    match source {
        crate::durable::observation::ResponseIdentitySource::Attempt { sequence } => {
            if *sequence > cut.read.journal {
                return Err(ConversationStoreError::InvalidReference(
                    "response outside source cut".into(),
                ));
            }
            let summary = observation_execution::summary_at_sequence(connection, *sequence)?
                .ok_or_else(|| storage("native response execution header unavailable"))?;
            if field == 1 {
                Ok((
                    format!(
                        "request-start:{}",
                        summary
                            .last_request_sequence
                            .ok_or_else(|| storage("response request identity unavailable"))?
                    ),
                    1,
                ))
            } else {
                Ok((
                    format!(
                        "journal:{}",
                        summary
                            .closing_identity_sequence
                            .ok_or_else(|| storage("response closing identity unavailable"))?
                    ),
                    0,
                ))
            }
        }
        crate::durable::observation::ResponseIdentitySource::Inherited { ordinal } => {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM inherited_response_provenance WHERE ordinal=?1 AND closing_ledger_position<=?2)",
                    params![ordinal,cut.ledger_through],
                    |row| row.get(0),
                )
                .map_err(|error| storage(error.to_string()))?;
            if !exists {
                return Err(ConversationStoreError::InvalidReference(
                    "inherited response identity unavailable".into(),
                ));
            }
            Ok((format!("inherited:{ordinal}"), field))
        }
    }
}

pub(in crate::durable::sqlite) fn response_identity_fields(
    connection: &Connection,
    cut: &crate::durable::observation::ObservationReadCut,
    source: &crate::durable::observation::ResponseIdentitySource,
) -> Result<Vec<crate::durable::observation::ResponseIdentityField>, ConversationStoreError> {
    let mut fields = Vec::new();
    for path in [
        vec!["closing_message_id".into()],
        vec!["retry_message_id".into()],
        vec!["origin".into(), "closing_message_id".into()],
    ] {
        if let Ok((key, field)) = response_identity_key(connection, cut, source, &path)
            && let Some(readable_end) =
                content::utf8_field_end(connection, "response_identity", &key, field)?
        {
            fields.push(crate::durable::observation::ResponseIdentityField { path, readable_end });
        }
    }
    Ok(fields)
}
