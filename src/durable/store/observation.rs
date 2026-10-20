//! Store contract for bounded native observation reads and readiness.

use crate::runtime::identity::{AttemptId, MessageId, RequestId};

use crate::durable::{ConversationStoreError, NativeTodoObservation, TranscriptCursor};

/// The bounded observation domain of the conversation Store: finite native
/// read cuts, display headers, and byte-range reads that never hydrate
/// unbounded history, plus the owner-published observation readiness
/// frontiers those reads are admitted against.
#[allow(clippy::missing_errors_doc)]
pub trait ObservationStore: Send + Sync + 'static {
    /// Finite typed facts from original native J rows at one authentic cut.
    fn observation_trace_facts(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        query: &crate::durable::observation::TraceFactQuery,
    ) -> Result<Vec<crate::durable::observation::TraceFactHeader>, ConversationStoreError>;
    /// None is original snapshot absence; an auxiliary summary hole is an error.
    fn observation_request_accepted_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
    ) -> Result<Option<crate::observation::content::CanonicalContentLocator>, ConversationStoreError>;
    fn observation_request_summary_at(
        &self,
        start_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::durable::observation::RequestReadSummary>, ConversationStoreError>;
    fn observation_request_predecessor_at(
        &self,
        start_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::TraceRequestPredecessorHeader>,
        ConversationStoreError,
    >;
    fn observation_request_retry_failure_at(
        &self,
        start_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::model::error::ModelErrorKind>, ConversationStoreError>;
    fn observation_trace_canonical_summary(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ledger_position: u64,
    ) -> Result<crate::durable::observation::TraceCanonicalSummary, ConversationStoreError>;
    /// Positive C result evidence is independent of a terminal J fact.
    fn observation_trace_tool_result(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<crate::durable::observation::TraceCanonicalSummary>, ConversationStoreError>;
    fn observation_trace_tool_proposal(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<crate::durable::observation::TraceToolProposalSummary, ConversationStoreError>;
    /// Same-original-request catalog selection; a missing optional relationship is unavailable.
    fn observation_trace_tool_definition(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        occurrence: &crate::events::types::ToolObservationOccurrence,
    ) -> Result<Option<crate::observation::trace::TraceFieldSource>, ConversationStoreError>;
    /// Advancing original adoption ordinal membership at the exact J/C cut.
    fn observation_trace_adoption(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::TraceAdoptionPage, ConversationStoreError>;
    fn observation_request_context_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        start_sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::TraceRequestContextPage, ConversationStoreError>;
    fn request_summary_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<crate::durable::observation::RequestObservationHeader, ConversationStoreError>;
    fn request_field_header_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError>;
    fn request_field_page_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError>;
    fn request_value_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    fn request_outcome_header_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::RequestObservationOutcome>,
        ConversationStoreError,
    >;
    fn request_outcome_read_at(
        &self,
        source_sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact immutable predecessor from native request start ordering.
    fn request_previous_identity(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError>;
    /// Request terminal evidence at native J, separate from immutable input detail.
    fn request_outcome_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<
        Option<crate::durable::observation::RequestObservationOutcome>,
        ConversationStoreError,
    >;
    /// Exact normalized request terminal structural bytes; only fixed allowlisted paths.
    fn request_outcome_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Finite immutable field descriptor without loading its original bytes.
    fn request_field_header(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError>;
    /// Exact native request start identity, without reading frozen snapshot bodies.
    fn request_identity_at(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<RequestId>, ConversationStoreError>;
    /// Frozen allowlisted request detail, captured at its original native start.
    fn request_summary_header(
        &self,
        request: &RequestId,
        through: u64,
    ) -> Result<crate::durable::observation::RequestObservationHeader, ConversationStoreError>;
    /// Indexed immediate children of one immutable request detail field.
    fn request_field_page(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError>;
    /// Exact immutable allowlisted structural JSON bytes, with actual bounded physical work.
    fn request_value_read(
        &self,
        request: &RequestId,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Validates immutable content against the server's observed native cut.
    fn validate_content_source_cut(
        &self,
        reference: &crate::observation::content::ContentRef,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<(), ConversationStoreError>;
    /// Original immutable owner field, selected by native provenance ordinal.
    fn observation_inherited_attempt_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        kind: crate::observation::turn::InheritedAttemptIdentityKind,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Raw native-page fixture oracle; production uses exact-cut numeric selectors.
    #[cfg(test)]
    fn observation_inherited_native_oracle(
        &self,
        messages: &[MessageId],
    ) -> Result<crate::durable::observation::InheritedObservation, ConversationStoreError>;
    /// Exact inherited actual model names by advancing native ordinal.
    fn observation_inherited_models(
        &self,
        response_ordinal: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::InheritedModelPage, ConversationStoreError>;
    /// Exact model JSON-string bytes under retained immutable lineage authority.
    fn observation_inherited_model_read(
        &self,
        response_ordinal: u64,
        model_ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact normalized terminal diagnostic JSON bytes under the captured native J.
    fn observation_failure_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact finite per-Attempt request/timing/usage aggregate at native J.
    fn response_identity_source(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript_cursor: u64,
    ) -> Result<Option<crate::durable::observation::ResponseIdentitySource>, ConversationStoreError>;
    fn inherited_response_summary(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ordinal: u64,
    ) -> Result<Option<crate::durable::observation::InheritedResponseHeader>, ConversationStoreError>;
    fn response_identity_fields(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
    ) -> Result<Vec<crate::durable::observation::ResponseIdentityField>, ConversationStoreError>;
    fn response_identity_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        source: &crate::durable::observation::ResponseIdentitySource,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Exact finite execution and member header from one original J row.
    /// Select the complete derived execution state through this authentic cut
    /// using the original numeric owner source; never an inline identity alias.
    fn observation_execution_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<crate::durable::observation::AttemptExecutionSummary, ConversationStoreError>;
    fn observation_members_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
    ) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError>;
    fn observation_attempt_summary(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<crate::durable::observation::AttemptExecutionSummary, ConversationStoreError>;
    fn todo_observation_head(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<NativeTodoObservation>, ConversationStoreError>;
    /// Discover a finite native source locator at the exact frozen J/L cut.
    fn observation_state_source(
        &self,
        domain: &str,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<String>, ConversationStoreError>;
    /// Finite source fields owned by current Goal, immutable canonical Todo, or Compaction J.
    fn observation_state_header(
        &self,
        domain: &str,
        source_id: &str,
        cut: &crate::durable::observation::ObservationReadCut,
        path: &[String],
    ) -> Result<crate::durable::observation::RequestObservationField, ConversationStoreError>;
    fn observation_state_page(
        &self,
        domain: &str,
        source_id: &str,
        cut: &crate::durable::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError>;
    fn observation_state_value_read(
        &self,
        domain: &str,
        source_id: &str,
        cut: &crate::durable::observation::ObservationReadCut,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Native distinct actual model names, by advancing bounded ordinal.
    /// Numeric original owner selection, without loading its original identity.
    fn observation_models_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AttemptModelNamePage, ConversationStoreError>;
    fn observation_model_read_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    fn observation_failure_read_at(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    fn observation_models_page(
        &self,
        attempt: &AttemptId,
        through: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AttemptModelNamePage, ConversationStoreError>;
    /// Exact immutable native model name JSON-string bytes, at most two finite rows.
    fn observation_model_value_read(
        &self,
        attempt: &AttemptId,
        through: u64,
        ordinal: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Last ordinary canonical User at this request's exact native Surface cut.
    fn observation_request_retry_locator_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<crate::observation::content::CanonicalContentLocator>, ConversationStoreError>;
    fn observation_request_retry_at(
        &self,
        source_sequence: u64,
        through: u64,
    ) -> Result<Option<MessageId>, ConversationStoreError>;
    fn observation_request_retry_locator(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<crate::observation::content::CanonicalContentLocator>, ConversationStoreError>;
    fn observation_request_retry(
        &self,
        request_id: &RequestId,
    ) -> Result<Option<MessageId>, ConversationStoreError>;
    /// Exact process count/first-position evidence, without member body reads.
    fn observation_member_counts(
        &self,
        message_id: &MessageId,
        through: u64,
    ) -> Result<Option<crate::durable::observation::AttemptMemberCounts>, ConversationStoreError>;
    /// Same-owner finite membership at the frozen original Journal cut.
    fn observation_attempt_members(
        &self,
        attempt: &AttemptId,
        through: u64,
    ) -> Result<crate::durable::observation::AttemptMemberCounts, ConversationStoreError>;
    fn observation_transcript_process(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript: u64,
    ) -> Result<Option<crate::observation::turn::TurnProcessView>, ConversationStoreError>;
    fn audit_block_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        transcript: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::AuditBlockHeaderPage, ConversationStoreError>;
    /// Projection settlement head at this exact published native source cut.
    fn publication_settlement_head(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<
        Vec<crate::observation::transcript::RuntimeClientPublicationSettlementRecord>,
        ConversationStoreError,
    >;
    fn observation_trace_total(&self, through: u64) -> Result<u64, ConversationStoreError>;
    /// Finite occupancy evidence at the original native reading's journal cut.
    fn observation_occupancy(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::observation::occupancy::RuntimeClientContextOccupancy>,
        ConversationStoreError,
    >;
    /// One immutable compaction or inherited reading's original model field.
    fn observation_occupancy_model_value_read(
        &self,
        sequence: Option<u64>,
        through: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Native-J scalar aggregate at an exact authoritative Journal frontier.
    fn observation_statistics(
        &self,
        through: u64,
    ) -> Result<crate::observation::statistics::ConversationStatistics, ConversationStoreError>;
    /// Local child statistics and exact working intervals from same-J headers.
    fn observation_agent_statistics(
        &self,
        through: u64,
    ) -> Result<crate::observation::statistics::AgentStatistics, ConversationStoreError>;
    /// Mutable pending headers at one exact native mutation cut, by exclusive sequence.
    fn pending_header_page(
        &self,
        cut: &crate::durable::reading::ConversationReadCut,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<crate::durable::observation::PendingHeaderPage, ConversationStoreError>;
    /// Pure byte range over an exact still-pending `UserMessage` structural encoding.
    fn pending_value_read(
        &self,
        cut: &crate::durable::reading::ConversationReadCut,
        sequence: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Native history/window positioning with only finite indexed display headers.
    fn read_window_headers(
        &self,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::durable::observation::ObservationWindow, ConversationStoreError>;
    /// Exact native T identity assigned to this J row, without decoding event bodies.
    fn event_transcript_cursor_at_sequence(
        &self,
        sequence: u64,
    ) -> Result<Option<TranscriptCursor>, ConversationStoreError>;
    /// Latest exact bounded native Journal display headers for named semantic families.
    /// Finite same-original-J kind; authorization never parses the original body.
    fn observation_event_kind(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<String>, ConversationStoreError>;
    fn observation_event_witness(
        &self,
        sequence: u64,
        through: u64,
    ) -> Result<Option<crate::durable::observation::ObservationEventWitness>, ConversationStoreError>;
    fn event_value_page(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::RequestObservationPage, ConversationStoreError>;
    fn event_value_read(
        &self,
        sequence: u64,
        through: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    fn latest_observation_events(
        &self,
        through: u64,
        kinds: &[String],
        attempt: Option<&AttemptId>,
        limit: usize,
    ) -> Result<Vec<crate::durable::observation::ObservationEventHeader>, ConversationStoreError>;
    /// Exact committed call occurrence under one native Attempt and frozen read cut.
    fn tool_call_occurrence_at(
        &self,
        call_id: &crate::runtime::identity::ToolCallId,
        attempt: &AttemptId,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<crate::message::types::ToolCallOccurrenceRef>, ConversationStoreError>;
    /// Canonical occurrence associations and finite result descriptors at one native cut.
    fn tool_association_headers(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError>;
    /// An exact native canonical descriptor seek, with no body deserialization.
    fn tool_association_headers_by_locator(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::ToolAssociationHeaderPage, ConversationStoreError>;
    /// Immutable canonical identity lookup from the authoritative Ledger,
    /// independent of optional display headers and body range indexes.
    fn ordinary_user_message(
        &self,
        locator: &crate::observation::content::CanonicalContentLocator,
    ) -> Result<bool, ConversationStoreError>;
    fn message_ledger_position(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<u64>, ConversationStoreError>;
    fn message_header(
        &self,
        message_id: &MessageId,
    ) -> Result<Option<crate::durable::observation::MessageHeader>, ConversationStoreError>;
    /// Advancing immutable canonical content-block descriptors at the captured native cut.
    fn message_block_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        message_id: &MessageId,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError>;
    fn message_block_header_page_by_locator(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageBlockHeaderPage, ConversationStoreError>;
    fn observation_tool_trace_sequence(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        ledger_position: u64,
        block_index: crate::message::types::ContentBlockIndex,
    ) -> Result<Option<u64>, ConversationStoreError>;
    /// Captures only native finite metadata; never reads Surface/body JSON.
    fn observation_read_cut(
        &self,
    ) -> Result<crate::durable::observation::ObservationReadCut, ConversationStoreError>;
    /// Canonical display headers; None selects the newest bounded tail.
    fn message_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::durable::observation::MessageHeaderPage, ConversationStoreError>;
    /// Transcript headers from its ordering spine, without source-body reads.
    fn transcript_header_page(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        before: Option<TranscriptCursor>,
        limit: usize,
    ) -> Result<crate::durable::observation::TranscriptHeaderPage, ConversationStoreError>;
    /// One exact snapshot of native reading authority.
    fn conversation_read_cut(
        &self,
    ) -> Result<crate::durable::reading::ConversationReadCut, ConversationStoreError>;
    /// Finite original turn display headers; omitted offset selects newest page.
    fn observation_turns(
        &self,
        offset: Option<u64>,
        limit: usize,
    ) -> Result<crate::observation::turn::ObservedTurnPage, ConversationStoreError>;
    /// Select one committed successful Tool delivery without loading its body.
    fn observation_file_delivery(
        &self,
        locator: &crate::observation::content::CanonicalContentLocator,
        index: u64,
    ) -> Result<crate::durable::observation::NativeFileDelivery, ConversationStoreError>;
    fn observation_deliveries(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        offset: u64,
        limit: usize,
    ) -> Result<crate::observation::delivery::RuntimeClientDeliveryHead, ConversationStoreError>;
    fn observation_delivery_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        locator: &crate::observation::content::CanonicalContentLocator,
        index: u64,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    fn conversation_turns(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<crate::durable::reading::ConversationTurnPage, ConversationStoreError>;
    /// The newest transcript page, or the page before a boundary, at the current cut.
    fn conversation_window(
        &self,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::durable::reading::DurableConversationWindow, ConversationStoreError>;
    /// Exact local execution owners of these canonical Assistant identities.
    fn transcript_attempts(
        &self,
        messages: &[MessageId],
        through: u64,
    ) -> Result<Vec<AttemptId>, ConversationStoreError>;
    /// Finite same-transaction Goal display header, without reading original body.
    fn goal_observation_view(
        &self,
    ) -> Result<crate::observation::goal::RuntimeClientGoalView, ConversationStoreError>;
    /// Bounded original provenance of one actual immutable activation item.
    fn observation_projection_provenance_read(
        &self,
        cut: &crate::durable::observation::ObservationReadCut,
        activation: u64,
        item: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError>;
    /// Finite original activation view; never hydrates checkpoint candidate bodies.
    fn observation_projection_head(
        &self,
        through: u64,
    ) -> Result<
        Option<crate::observation::head::RuntimeClientProjectionHeadView>,
        ConversationStoreError,
    >;
    /// Mark only an existing native owner's committed publication range ready.
    /// Readiness does not authorize model-output bytes or acknowledge a client.
    fn mark_observation_ready(&self, first: u64, last: u64) -> Result<(), ConversationStoreError>;
    fn mark_observation_direct(&self, first: u64, last: u64) -> Result<(), ConversationStoreError>;
    /// Last contiguous ready Journal position within a committed source range.
    fn observation_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError>;
    /// Publish one exact native transcript position; removed pending positions are retired.
    fn mark_observation_transcript_ready(&self, cursor: u64) -> Result<(), ConversationStoreError>;
    /// Publish only canonical positions owned by this exact atomic C ledger range.
    fn mark_canonical_transcript_ready(
        &self,
        first_ledger: u64,
        last_ledger: u64,
    ) -> Result<(), ConversationStoreError>;
    /// Contiguous owner-published prefix across retained native transcript positions.
    fn observation_transcript_ready_frontier(
        &self,
        after: u64,
        through: u64,
    ) -> Result<u64, ConversationStoreError>;
    /// Capture the native read cut and owner-published T prefix in one read snapshot.
    fn transcript_source_ready(
        &self,
    ) -> Result<crate::durable::observation::TranscriptSourceReady, ConversationStoreError>;
    /// Latest byte descriptor from a retained source, without loading its body.
    /// Exact committed frame/immutable revision evidence, never the latest staged head.
    /// Reads a native P-indexed block page at an explicitly observed release revision.
    fn live_block_header_page(
        &self,
        source: &crate::observation::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::observation::content::LiveBlockHeaderPage, ConversationStoreError>;
    fn content_descriptor_at(
        &self,
        source: &crate::observation::content::ContentSource,
        representation: crate::observation::content::ContentRepresentation,
        revision: u64,
    ) -> Result<Option<crate::observation::content::ContentRef>, ConversationStoreError>;
    fn content_descriptor(
        &self,
        source: &crate::observation::content::ContentSource,
        representation: crate::observation::content::ContentRepresentation,
    ) -> Result<Option<crate::observation::content::ContentRef>, ConversationStoreError>;
    fn live_tool_header_page(
        &self,
        source: &crate::observation::content::ContentSource,
        through_revision: u64,
        offset: u64,
        limit: usize,
    ) -> Result<crate::observation::content::LiveBlockHeaderPage, ConversationStoreError>;
    /// Pure bounded physical range read. Target authorization belongs to the caller.
    fn content_read(
        &self,
        reference: &crate::observation::content::ContentRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<crate::observation::content::ContentChunk, ConversationStoreError>;
}
