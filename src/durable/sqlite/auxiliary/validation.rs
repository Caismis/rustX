//! Pure validation of auxiliary inputs, terminal envelopes, settlement pairs, and fact identities.

use super::{
    AUXILIARY_CALL_START_ENCODED_MAX, AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX,
    AUXILIARY_REQUEST_ID_BYTES_MAX, AUXILIARY_REQUEST_MODEL_BYTES_MAX,
    AUXILIARY_REQUEST_START_ENCODED_MAX, AUXILIARY_SEQUENCE_ENCODING_DIGITS_MAX,
    AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX, AuxiliaryAuditFact, AuxiliaryAuditRecord,
    AuxiliaryCallAudit, AuxiliaryCallId, AuxiliaryCallOutcome, AuxiliaryCallStart,
    AuxiliaryFinishReason, AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal,
    AuxiliaryRequestAudit, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
    AuxiliaryRequestTerminal, AuxiliaryUsageCompleteness, BTreeSet, ConversationId,
    ConversationStoreError, EVENT_SCHEMA_VERSION, EventId, GenerationEvidence, ModelRequestOwner,
    ModelUsage, RequestId, RuntimeEvent, RuntimeEventEnvelope, audit_record_bytes, storage,
};

#[derive(Default)]
pub(super) struct BoundedWriter {
    pub(super) bytes: usize,
    pub(super) maximum: usize,
    pub(super) exceeded: bool,
}

impl std::io::Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let Some(total) = self.bytes.checked_add(buffer.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::other("encoded byte count overflow"));
        };
        if total > self.maximum {
            self.exceeded = true;
            return Err(std::io::Error::other("encoded byte bound exceeded"));
        }
        self.bytes = total;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn bounded_encoded_len<T: serde::Serialize + ?Sized>(
    value: &T,
    maximum: usize,
    name: &str,
) -> Result<usize, ConversationStoreError> {
    let mut writer = BoundedWriter {
        bytes: 0,
        maximum,
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        if writer.exceeded {
            return Err(ConversationStoreError::ResourceBoundExceeded(format!(
                "{name} exceeds its {maximum}-byte encoded bound"
            )));
        }
        return Err(storage(format!("encode {name}")));
    }
    Ok(writer.bytes)
}

pub(super) fn validate_call_start_input(
    start: &AuxiliaryCallStart,
) -> Result<(), ConversationStoreError> {
    let call_bytes = start.call_id.as_str().len();
    if !(1..=64).contains(&call_bytes) {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary CallId must contain 1..=64 bytes".into(),
        ));
    }
    if start.selected_message_ids.len() > AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX {
        return Err(ConversationStoreError::ResourceBoundExceeded(format!(
            "auxiliary Call may select at most {AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX} MessageIds"
        )));
    }
    if start
        .selected_message_ids
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != start.selected_message_ids.len()
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary selected MessageIds must be unique".into(),
        ));
    }
    if matches!(start.input, AuxiliaryInputEvidence::Temporary)
        && !start.selected_message_ids.is_empty()
    {
        return Err(ConversationStoreError::InvalidReference(
            "temporary auxiliary input cannot name selected historical MessageIds".into(),
        ));
    }
    bounded_encoded_len(
        start,
        AUXILIARY_CALL_START_ENCODED_MAX,
        "auxiliary Call start metadata",
    )?;
    audit_record_bytes(&AuxiliaryAuditRecord {
        audit_offset: u64::MAX,
        journal_sequence: u64::MAX,
        fact: AuxiliaryAuditFact::CallSettled(AuxiliaryCallAudit {
            call_id: start.call_id.clone(),
            purpose: start.purpose.clone(),
            trigger: start.trigger.clone(),
            business_source: start.business_source.clone(),
            input: start.input.clone(),
            semantic_input_fingerprint: start.semantic_input_fingerprint.clone(),
            invocation_fingerprint: start.invocation_fingerprint.clone(),
            selected_message_ids: start.selected_message_ids.clone(),
            started_sequence: u64::MAX,
            settled_sequence: Some(u64::MAX),
            outcome: Some(AuxiliaryCallOutcome::Completed),
        }),
    })?;
    Ok(())
}

pub(super) fn validate_request_start_input(
    start: &AuxiliaryRequestStart,
) -> Result<(), ConversationStoreError> {
    if start.ordinal >= 3 {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call start allowance is three".into(),
        ));
    }

    if start.request_id.as_str().is_empty()
        || start.request_id.as_str().len() > AUXILIARY_REQUEST_ID_BYTES_MAX
    {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary RequestId must contain 1..={AUXILIARY_REQUEST_ID_BYTES_MAX} bytes"
        )));
    }
    if start.model.is_empty() || start.model.len() > AUXILIARY_REQUEST_MODEL_BYTES_MAX {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary request model name must contain 1..={AUXILIARY_REQUEST_MODEL_BYTES_MAX} bytes"
        )));
    }
    let maximum_usage = ModelUsage {
        input_tokens: Some(u64::MAX),
        output_tokens: Some(u64::MAX),
        total_tokens: Some(u64::MAX),
        details: Some(crate::model::types::UsageDetails {
            reasoning_tokens: Some(u64::MAX),
            cached_input_tokens: Some(u64::MAX),
        }),
    };
    let worst_case = AuxiliaryAuditRecord {
        audit_offset: u64::MAX,
        journal_sequence: u64::MAX,
        fact: AuxiliaryAuditFact::RequestSettled(AuxiliaryRequestAudit {
            call_id: start.call_id.clone(),
            ordinal: start.ordinal,
            request_id: start.request_id.clone(),
            model: start.model.clone(),
            started_sequence: u64::MAX,
            terminal_sequence: Some(u64::MAX),
            local_terminal: Some(AuxiliaryRequestTerminal::Completed),
            normalized_failure_kind: Some(
                crate::model::error::ModelErrorKind::GenerationBudgetExceeded,
            ),
            generation: Some(GenerationEvidence {
                dispatch_after_start_ms: Some(u64::MAX),
                first_output_ms: Some(u64::MAX),
                last_output_ms: Some(u64::MAX),
                terminal_ms: u64::MAX,
            }),
            usage: Some(maximum_usage.clone()),
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id: start.request_id.clone(),
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: AuxiliaryFinishReason::ContentFilter,
                }),
                usage: Some(maximum_usage),
                usage_completeness: AuxiliaryUsageCompleteness::Complete,
                remote_execution_uncertainty: true,
            }),
        }),
    };
    audit_record_bytes(&worst_case)?;
    bounded_encoded_len(
        start,
        AUXILIARY_REQUEST_START_ENCODED_MAX,
        "auxiliary request start metadata",
    )?;
    Ok(())
}

pub(super) fn call_started_event_id(call_id: &AuxiliaryCallId) -> EventId {
    EventId::new(format!("auxiliary-call-started:{call_id}"))
}

pub(super) fn request_started_event_id(request_id: &RequestId) -> EventId {
    EventId::new(format!("auxiliary-request-started:{request_id}"))
}

pub(super) fn call_start_matches(event: &RuntimeEventEnvelope, start: &AuxiliaryCallStart) -> bool {
    event.attempt_id.is_none()
        && event.turn_id.is_none()
        && matches!(
            &event.event,
            RuntimeEvent::AuxiliaryCallStarted {
                call_id,
                purpose,
                trigger,
                business_source,
                input,
                semantic_input_fingerprint,
                invocation_fingerprint,
                selected_message_ids,
            } if call_id == &start.call_id
                && *purpose == start.purpose
                && *trigger == start.trigger
                && business_source == &start.business_source
                && input == &start.input
                && semantic_input_fingerprint == &start.semantic_input_fingerprint
                && invocation_fingerprint == &start.invocation_fingerprint
                && selected_message_ids == &start.selected_message_ids
        )
}

pub(super) fn request_start_matches(
    event: &RuntimeEventEnvelope,
    start: &AuxiliaryRequestStart,
) -> bool {
    event.attempt_id.is_none()
        && event.turn_id.is_none()
        && matches!(
            &event.event,
            RuntimeEvent::ModelRequestStarted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                model,
            } if call_id == &start.call_id
                && *ordinal == start.ordinal
                && request_id == &start.request_id
                && model == &start.model
        )
}

pub(super) fn terminal_kind(event: &RuntimeEvent) -> Option<&'static str> {
    match event {
        RuntimeEvent::ModelRequestCompleted { .. } => Some("completed"),
        RuntimeEvent::ModelRequestFailed { .. } => Some("failed"),
        _ => None,
    }
}

pub(super) fn outcome_name(outcome: AuxiliaryCallOutcome) -> &'static str {
    match outcome {
        AuxiliaryCallOutcome::Completed => "completed",
        AuxiliaryCallOutcome::Failed => "failed",
        AuxiliaryCallOutcome::Cancelled => "cancelled",
    }
}

pub(super) fn call_outcome_for_terminal(event: &RuntimeEvent) -> Option<&[AuxiliaryCallOutcome]> {
    match event {
        RuntimeEvent::ModelRequestCompleted { .. } => Some(&[AuxiliaryCallOutcome::Completed]),
        RuntimeEvent::ModelRequestFailed { .. } => Some(&[
            AuxiliaryCallOutcome::Failed,
            AuxiliaryCallOutcome::Cancelled,
        ]),
        _ => None,
    }
}

pub(super) fn decimal_digits(mut value: u64) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

pub(super) fn validate_terminal_envelope_shape(
    event: &RuntimeEventEnvelope,
    conversation_id: &ConversationId,
    require_unallocated_sequence: bool,
) -> Result<(), ConversationStoreError> {
    if event.schema_version != EVENT_SCHEMA_VERSION {
        return Err(ConversationStoreError::InvalidReference(format!(
            "auxiliary terminal schema {} is not {}",
            event.schema_version, EVENT_SCHEMA_VERSION
        )));
    }
    if event.conversation_id != *conversation_id {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: conversation_id.clone(),
            requested: event.conversation_id.clone(),
        });
    }
    if event.attempt_id.is_some() || event.turn_id.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement envelopes must not carry an Attempt or Turn".into(),
        ));
    }
    if event.tool_occurrence.is_some() || event.observation.is_some() {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement envelopes cannot carry Tool or observation evidence".into(),
        ));
    }
    if require_unallocated_sequence && event.sequence != 0 {
        return Err(ConversationStoreError::InvalidReference(
            "a new auxiliary settlement must leave Journal sequence unallocated".into(),
        ));
    }
    if event.event_id != crate::durable::auxiliary::settlement_event_id(&event.event)? {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary terminal has a noncanonical event identity".into(),
        ));
    }
    if let RuntimeEvent::ModelRequestCompleted {
        generation: Some(generation),
        ..
    }
    | RuntimeEvent::ModelRequestFailed {
        generation: Some(generation),
        ..
    } = &event.event
        && !crate::model::auxiliary::generation_is_bounded(generation)
    {
        return Err(ConversationStoreError::ResourceBoundExceeded(
            "auxiliary generation timing exceeds the common observation integer domain".into(),
        ));
    }
    let reserved_sequence_digits =
        AUXILIARY_SEQUENCE_ENCODING_DIGITS_MAX.saturating_sub(decimal_digits(event.sequence));
    let admission_maximum =
        AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX.saturating_sub(reserved_sequence_digits);
    bounded_encoded_len(event, admission_maximum, "auxiliary settlement envelope")?;
    Ok(())
}

pub(super) fn validate_request_evidence(
    evidence: &AuxiliaryRequestEvidence,
    request_terminal: &RuntimeEventEnvelope,
) -> Result<(), ConversationStoreError> {
    let (request_id, local_completed, local_finish_reason, usage) = match &request_terminal.event {
        RuntimeEvent::ModelRequestCompleted {
            request_id,
            finish_reason,
            usage,
            ..
        } => (request_id, true, Some(finish_reason), usage),
        RuntimeEvent::ModelRequestFailed {
            request_id, usage, ..
        } => (request_id, false, None, usage),
        _ => {
            return Err(ConversationStoreError::InvalidReference(
                "auxiliary request evidence requires a request terminal".into(),
            ));
        }
    };
    if &evidence.request_id != request_id {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement request evidence names another RequestId".into(),
        ));
    }
    if evidence.usage != *usage {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary settlement Usage disagrees with the physical terminal".into(),
        ));
    }
    match evidence.usage_completeness {
        AuxiliaryUsageCompleteness::Complete | AuxiliaryUsageCompleteness::Partial
            if evidence.usage.is_none() =>
        {
            return Err(ConversationStoreError::InvalidReference(
                "observed auxiliary Usage completeness requires Usage evidence".into(),
            ));
        }
        AuxiliaryUsageCompleteness::Unreported if evidence.usage.is_some() => {
            return Err(ConversationStoreError::InvalidReference(
                "unreported auxiliary Usage cannot carry a Usage value".into(),
            ));
        }
        _ => {}
    }
    if local_completed {
        match &evidence.observed_provider_terminal {
            None => {
                return Err(ConversationStoreError::InvalidReference(
                    "a locally completed auxiliary request requires an observed provider completion"
                        .into(),
                ));
            }
            Some(AuxiliaryObservedProviderTerminal::Failed { .. }) => {
                return Err(ConversationStoreError::InvalidReference(
                    "a locally completed auxiliary request cannot record provider failure as its terminal"
                        .into(),
                ));
            }
            Some(AuxiliaryObservedProviderTerminal::Completed { finish_reason })
                if Some(*finish_reason) != local_finish_reason.map(AuxiliaryFinishReason::from) =>
            {
                return Err(ConversationStoreError::InvalidReference(
                    "observed provider completion kind disagrees with the locally completed auxiliary request"
                        .into(),
                ));
            }
            Some(AuxiliaryObservedProviderTerminal::Completed { .. }) => {}
        }
    }
    Ok(())
}

pub(super) fn validate_settlement_pair(
    conversation_id: &ConversationId,
    request_terminal: &RuntimeEventEnvelope,
    call_settled: &RuntimeEventEnvelope,
    require_unallocated_sequence: bool,
) -> Result<(), ConversationStoreError> {
    validate_terminal_envelope_shape(
        request_terminal,
        conversation_id,
        require_unallocated_sequence,
    )?;
    validate_terminal_envelope_shape(call_settled, conversation_id, require_unallocated_sequence)?;
    if request_terminal.conversation_id != call_settled.conversation_id {
        return Err(ConversationStoreError::ConversationIdMismatch {
            stored: request_terminal.conversation_id.clone(),
            requested: call_settled.conversation_id.clone(),
        });
    }
    let Some(ModelRequestOwner::Auxiliary {
        call_id: request_call,
        ..
    }) = request_terminal.event.model_request_owner()
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request settlement requires an auxiliary-owned request terminal".into(),
        ));
    };
    let RuntimeEvent::AuxiliaryCallSettled {
        call_id: settled_call,
        outcome,
        request_evidence,
    } = &call_settled.event
    else {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request settlement requires an AuxiliaryCallSettled fact".into(),
        ));
    };
    if request_call != settled_call {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary request and Call settlement name different Calls".into(),
        ));
    }
    if !call_outcome_for_terminal(&request_terminal.event)
        .is_some_and(|outcomes| outcomes.contains(outcome))
    {
        return Err(ConversationStoreError::InvalidReference(
            "auxiliary Call outcome is incompatible with its request terminal".into(),
        ));
    }
    let evidence = request_evidence.as_ref().ok_or_else(|| {
        ConversationStoreError::InvalidReference(
            "final auxiliary request settlement requires request evidence".into(),
        )
    })?;
    validate_request_evidence(evidence, request_terminal)
}

pub(super) fn same_terminal_envelope(
    stored: &RuntimeEventEnvelope,
    expected: &RuntimeEventEnvelope,
) -> bool {
    (expected.sequence == 0 || stored.sequence == expected.sequence)
        && stored.schema_version == expected.schema_version
        && stored.event_id == expected.event_id
        && stored.conversation_id == expected.conversation_id
        && stored.attempt_id == expected.attempt_id
        && stored.turn_id == expected.turn_id
        && stored.timestamp == expected.timestamp
        && stored.event == expected.event
}
