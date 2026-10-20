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

#[cfg(test)]
mod tests {
    use crate::durable::auxiliary::auxiliary_settlement_envelope;
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
    use crate::local_runtime::session::tests::open_catalog;
    use crate::model::ModelRequestOwner;
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart,
        AuxiliaryFingerprint, AuxiliaryFinishReason, AuxiliaryInputEvidence,
        AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
        AuxiliaryRequestTerminal, AuxiliaryUsageCompleteness,
    };
    use crate::model::generation_evidence::GenerationEvidence;
    use crate::model::{ModelFinishReason, ModelUsage};
    use crate::runtime::identity::{
        AuxiliaryCallId, ConversationId, RequestId, SystemUuidV7Generator,
    };
    use crate::session_archive::SessionArchiveProducer;
    use chrono::Utc;
    use std::io::Read;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio_util::sync::CancellationToken;

    fn fingerprint(byte: char) -> AuxiliaryFingerprint {
        AuxiliaryFingerprint::new(byte.to_string().repeat(64)).unwrap()
    }

    fn start_auxiliary(store: &SqliteConversationStore) -> (AuxiliaryCallId, RequestId) {
        let call_id = AuxiliaryCallId::generate();
        let request_id = RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap();
        store
            .commit_auxiliary_call_start(
                &AuxiliaryCallStart {
                    call_id: call_id.clone(),
                    purpose: AuxiliaryAttribution::new("archive-regression").unwrap(),
                    trigger: AuxiliaryAttribution::new("test").unwrap(),
                    business_source: AuxiliaryBusinessSource::Independent,
                    input: AuxiliaryInputEvidence::Historical {
                        cut: store.compaction_resource_cut().unwrap(),
                    },
                    semantic_input_fingerprint: fingerprint('a'),
                    invocation_fingerprint: fingerprint('b'),
                    selected_message_ids: Vec::new(),
                },
                Utc::now(),
            )
            .unwrap();
        store
            .commit_auxiliary_request_start(
                &AuxiliaryRequestStart {
                    call_id: call_id.clone(),
                    ordinal: 0,
                    request_id: request_id.clone(),
                    model: "fixture/auxiliary-model".into(),
                },
                Utc::now(),
            )
            .unwrap();
        (call_id, request_id)
    }

    fn completed_settlement(
        conversation_id: &ConversationId,
        call_id: &AuxiliaryCallId,
        request_id: &RequestId,
    ) -> (RuntimeEventEnvelope, RuntimeEventEnvelope) {
        let usage = ModelUsage {
            input_tokens: 41,
            output_tokens: 7,
            total_tokens: 48,
            details: None,
        };
        let request_terminal = auxiliary_settlement_envelope(
            conversation_id.clone(),
            Utc::now(),
            RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary {
                    call_id: call_id.clone(),
                    ordinal: 0,
                },
                request_id: request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: Some(usage.clone()),
                generation: Some(GenerationEvidence {
                    dispatch_after_start_ms: Some(3),
                    first_output_ms: Some(8),
                    last_output_ms: Some(21),
                    terminal_ms: 25,
                }),
            },
        )
        .unwrap();
        let call_settled = auxiliary_settlement_envelope(
            conversation_id.clone(),
            Utc::now(),
            RuntimeEvent::AuxiliaryCallSettled {
                call_id: call_id.clone(),
                outcome: AuxiliaryCallOutcome::Completed,
                request_evidence: Some(AuxiliaryRequestEvidence {
                    request_id: request_id.clone(),
                    observed_provider_terminal: Some(
                        AuxiliaryObservedProviderTerminal::Completed {
                            finish_reason: AuxiliaryFinishReason::Stop,
                        },
                    ),
                    usage: Some(usage),
                    usage_completeness: AuxiliaryUsageCompleteness::Complete,
                    remote_execution_uncertainty: false,
                }),
            },
        )
        .unwrap();
        (request_terminal, call_settled)
    }

    fn records(bytes: &[u8]) -> Vec<serde_json::Value> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Exercise real capture, concurrent settlement and archive contents.
    fn archive_preflight_keeps_captured_auxiliary_start_when_settlement_races() {
        let (root, catalog, _) = open_catalog();
        let canonical_root = root.path().canonicalize().unwrap();
        let session = catalog.list_page(None, 0, 1).unwrap().sessions[0]
            .id
            .clone();
        let (node, _) = catalog.lineage(&session, None).unwrap();
        let database = catalog.database_path(&session, &node.conversation_id);
        let writer =
            SqliteConversationStore::open(node.conversation_id.clone(), &database).unwrap();
        let (call_id, request_id) = start_auxiliary(&writer);
        let (request_terminal, call_settled) =
            completed_settlement(&node.conversation_id, &call_id, &request_id);

        let settler =
            SqliteConversationStore::open(node.conversation_id.clone(), &database).unwrap();
        let hook_executed = Arc::new(AtomicBool::new(false));
        let executed = hook_executed.clone();
        SqliteConversationStore::install_after_request_row_read_hook(
            request_id.clone(),
            move || {
                settler
                    .commit_auxiliary_request_settlement(request_terminal, call_settled)
                    .unwrap();
                executed.store(true, Ordering::SeqCst);
            },
        );

        let cut =
            SessionArchiveProducer::prepare(&canonical_root, &session, &CancellationToken::new())
                .expect("archive preflight must accept the captured start snapshot");
        assert!(
            hook_executed.load(Ordering::SeqCst),
            "the post-row-read settlement hook must execute"
        );

        let mut bytes = Vec::new();
        cut.write_test(&mut bytes).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut audit = Vec::new();
        zip.by_name(&format!(
            "sessions/{}/auxiliary_audit.jsonl",
            node.conversation_id
        ))
        .unwrap()
        .read_to_end(&mut audit)
        .unwrap();
        let audit = records(&audit);
        assert_eq!(
            audit
                .iter()
                .map(|row| row["event"]["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["auxiliary_call_started", "model_request_started"]
        );
        assert_eq!(
            audit[1]["event"]["request_id"],
            serde_json::json!(request_id)
        );

        let mut journal = Vec::new();
        zip.by_name(&format!("sessions/{}/journal.jsonl", node.conversation_id))
            .unwrap()
            .read_to_end(&mut journal)
            .unwrap();
        let auxiliary_journal = records(&journal)
            .into_iter()
            .filter(|row| {
                row["event"].get("call_id").is_some()
                    || row["event"]["owner"]["type"] == "auxiliary"
            })
            .collect::<Vec<_>>();
        assert_eq!(auxiliary_journal.len(), 2);
        assert_eq!(
            auxiliary_journal
                .iter()
                .map(|row| row["event"]["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["auxiliary_call_started", "model_request_started"]
        );

        let mut generations = Vec::new();
        zip.by_name(&format!(
            "sessions/{}/generations.jsonl",
            node.conversation_id
        ))
        .unwrap()
        .read_to_end(&mut generations)
        .unwrap();
        assert!(generations.is_empty());

        let current_cut = writer.auxiliary_audit_cut().unwrap();
        let request = writer
            .load_auxiliary_request(&request_id, current_cut)
            .unwrap()
            .unwrap();
        assert_eq!(
            request.local_terminal,
            Some(AuxiliaryRequestTerminal::Completed)
        );
        assert!(request.terminal_sequence.is_some());
        let call = writer
            .load_auxiliary_call(&call_id, current_cut)
            .unwrap()
            .unwrap();
        assert_eq!(call.outcome, Some(AuxiliaryCallOutcome::Completed));
    }
}
