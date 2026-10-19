//! Cut-bound auxiliary audit export. This is inspection, never recovery input.
use super::{ConversationCut, Record};
use crate::durable::ConversationStore;
use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
use crate::model::ModelRequestOwner;
use crate::model::auxiliary::{AuxiliaryAuditCut, AuxiliaryRequestTerminal};
use std::io::{self, Write};
use tokio_util::sync::CancellationToken;

impl ConversationCut {
    pub(super) fn write_auxiliary_audit(
        &self,
        writer: &mut impl Write,
        cancel: &CancellationToken,
    ) -> io::Result<()> {
        self.each(
            crate::durable::sqlite::archive::Authority::Journal,
            self.manifest.frontiers.journal,
            cancel,
            |record| {
                if let Record::Journal(envelope) = record
                    && envelope.event.auxiliary_call_id().is_some()
                {
                    serde_json::to_writer(&mut *writer, &super::projection::journal(&envelope))
                        .map_err(super::error)?;
                    writer.write_all(b"\n")?;
                }
                Ok(())
            },
        )
    }

    pub(super) fn write_inherited_auxiliary_audit(
        &self,
        writer: &mut impl Write,
        cancel: &CancellationToken,
    ) -> io::Result<()> {
        for record in self.store.archive_inherited_auxiliary_audit_rows()? {
            super::check_cancel(cancel)?;
            let record = super::projection::inherited_auxiliary_record(record)?;
            serde_json::to_writer(&mut *writer, &record).map_err(super::error)?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    }

    /// Validate original facts against mandatory native indexes at the archive cut.
    /// Optional observation headers and current post-cut state are not evidence.
    pub(super) fn validate_auxiliary_fact(
        &self,
        envelope: &RuntimeEventEnvelope,
    ) -> io::Result<()> {
        let cut = AuxiliaryAuditCut {
            journal: u64::try_from(self.manifest.frontiers.journal).map_err(super::error)?,
        };
        let valid = match &envelope.event {
            RuntimeEvent::AuxiliaryCallStarted {
                call_id,
                purpose,
                trigger,
                business_source,
                input,
                semantic_input_fingerprint,
                invocation_fingerprint,
                selected_message_ids,
            } => self
                .store
                .load_auxiliary_call(call_id, cut)
                .map_err(super::error)?
                .is_some_and(|call| {
                    call.started_sequence == envelope.sequence
                        && call.purpose == *purpose
                        && call.trigger == *trigger
                        && call.business_source == *business_source
                        && call.input == *input
                        && call.semantic_input_fingerprint == *semantic_input_fingerprint
                        && call.invocation_fingerprint == *invocation_fingerprint
                        && call.selected_message_ids == *selected_message_ids
                }),
            RuntimeEvent::AuxiliaryCallSettled {
                call_id, outcome, ..
            } => self
                .store
                .load_auxiliary_call(call_id, cut)
                .map_err(super::error)?
                .is_some_and(|call| {
                    call.settled_sequence == Some(envelope.sequence)
                        && call.outcome == Some(*outcome)
                }),
            RuntimeEvent::ModelRequestStarted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                model,
            } => self
                .store
                .load_auxiliary_request(request_id, cut)
                .map_err(super::error)?
                .is_some_and(|request| {
                    request.call_id == *call_id
                        && request.ordinal == *ordinal
                        && request.model == *model
                        && request.started_sequence == envelope.sequence
                }),
            RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                usage,
                ..
            } => self
                .store
                .load_auxiliary_request(request_id, cut)
                .map_err(super::error)?
                .is_some_and(|request| {
                    request.call_id == *call_id
                        && request.ordinal == *ordinal
                        && request.terminal_sequence == Some(envelope.sequence)
                        && request.local_terminal == Some(AuxiliaryRequestTerminal::Completed)
                        && request.usage == *usage
                }),
            RuntimeEvent::ModelRequestFailed {
                owner: ModelRequestOwner::Auxiliary { call_id, ordinal },
                request_id,
                usage,
                ..
            } => self
                .store
                .load_auxiliary_request(request_id, cut)
                .map_err(super::error)?
                .is_some_and(|request| {
                    request.call_id == *call_id
                        && request.ordinal == *ordinal
                        && request.terminal_sequence == Some(envelope.sequence)
                        && request.local_terminal == Some(AuxiliaryRequestTerminal::Failed)
                        && request.usage == *usage
                }),
            _ => return Ok(()),
        };
        if valid {
            Ok(())
        } else {
            Err(super::error(
                "auxiliary archive fact disagrees with native audit authority",
            ))
        }
    }
}
