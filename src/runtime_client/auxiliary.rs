//! Bounded auxiliary Call/request audit pages at one captured Journal cut.

use super::checkpoint::{
    CheckpointPage, CheckpointPageItem, CheckpointReadCut, CollectionFrontier, bounded_bytes,
    structural_value,
};
use super::types::{RuntimeClientCursor, RuntimeClientError};
use crate::durable::ConversationStore;
use crate::model::auxiliary::{
    AUXILIARY_AUDIT_PAGE_LIMIT_MAX, AUXILIARY_AUDIT_RECORD_ENCODED_MAX, AuxiliaryAuditCut,
    AuxiliaryAuditFact, AuxiliaryAuditRecord,
};

pub(crate) const DOMAIN: &str = "auxiliary.audit";
const PROTOCOL_INTEGER_MAX: u64 = 9_007_199_254_740_991;

fn identity(fact: &AuxiliaryAuditFact) -> String {
    match fact {
        AuxiliaryAuditFact::CallStarted(call) => {
            format!("auxiliary:call:{}:started", call.call_id)
        }
        AuxiliaryAuditFact::RequestStarted(request) => {
            format!("auxiliary:request:{}:started", request.request_id)
        }
        AuxiliaryAuditFact::RequestSettled(request) => {
            format!("auxiliary:request:{}:settled", request.request_id)
        }
        AuxiliaryAuditFact::CallSettled(call) => {
            format!("auxiliary:call:{}:settled", call.call_id)
        }
    }
}

fn complete_item(record: &AuxiliaryAuditRecord) -> Result<CheckpointPageItem, &'static str> {
    let item_identity = identity(&record.fact);
    complete_item_with_identity(record, item_identity)
}

fn complete_item_with_identity(
    record: &AuxiliaryAuditRecord,
    item_identity: String,
) -> Result<CheckpointPageItem, &'static str> {
    bounded_bytes(record, AUXILIARY_AUDIT_RECORD_ENCODED_MAX)
        .ok_or("auxiliary audit record exceeds its finite 12 KiB structural bound")?;
    let value = structural_value(record)
        .ok_or("auxiliary audit record is not representable in structural JSON")?;
    let item = CheckpointPageItem {
        body_complete: Some(true),
        key: None,
        offset: record.audit_offset - 1,
        identity: item_identity,
        value: Some(value),
        content: Vec::new(),
        deferred: false,
    };
    structural_value(&item)
        .ok_or("auxiliary audit page item exceeds its finite 16 KiB structural bound")?;
    Ok(item)
}

#[allow(clippy::too_many_lines)] // Validate one bounded native page before publication.
pub(crate) fn page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    if !(1..=AUXILIARY_AUDIT_PAGE_LIMIT_MAX).contains(&limit) || offset > PROTOCOL_INTEGER_MAX {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid auxiliary audit page bounds".into(),
        });
    }
    if cut.conversation_id != *store.conversation_id() {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    if cut.source_cut.as_ref().is_some_and(|source| {
        source.read.conversation_id != cut.conversation_id || source.read.journal != cut.through
    }) {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    // The audit uses only Journal authority. A missing optional Source cut is
    // independent, but a present Source cut must still identify this exact J.
    let audit_cut = AuxiliaryAuditCut {
        journal: cut.through,
    };
    let source_page = store
        .read_auxiliary_audit(audit_cut, (offset > 0).then_some(offset), limit)
        .map_err(RuntimeClientError::observation_store_error)?;
    if source_page.cut != audit_cut {
        return Err(RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page returned a different cut".into(),
        });
    }
    if source_page.total > PROTOCOL_INTEGER_MAX
        || source_page.records.len() > limit
        || source_page.records.len() > AUXILIARY_AUDIT_PAGE_LIMIT_MAX
    {
        return Err(RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page exceeded its encoded item or protocol bounds".into(),
        });
    }
    let loaded = u64::try_from(source_page.records.len()).map_err(|_| {
        RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page item count overflow".into(),
        }
    })?;
    let remaining = source_page.total.checked_sub(offset).ok_or_else(|| {
        RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page offset is after its total".into(),
        }
    })?;
    let page_limit =
        u64::try_from(limit).map_err(|_| RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page limit overflow".into(),
        })?;
    if loaded != remaining.min(page_limit) {
        return Err(RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page did not materialize its dense total".into(),
        });
    }
    let end =
        offset
            .checked_add(loaded)
            .ok_or_else(|| RuntimeClientError::ObservationUnavailable {
                message: "auxiliary audit page offset overflow".into(),
            })?;
    if end > source_page.total {
        return Err(RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page exceeded its total".into(),
        });
    }
    let expected_next = (end < source_page.total).then_some(end);
    if source_page.next_offset != expected_next
        || source_page
            .next_offset
            .is_some_and(|next| next > PROTOCOL_INTEGER_MAX)
    {
        return Err(RuntimeClientError::ObservationUnavailable {
            message: "auxiliary audit page has an invalid continuation offset".into(),
        });
    }
    let mut items = Vec::with_capacity(source_page.records.len());
    let mut previous_journal = 0;
    for record in source_page.records {
        let item_index =
            u64::try_from(items.len()).map_err(|_| RuntimeClientError::ObservationUnavailable {
                message: "auxiliary audit item count overflow".into(),
            })?;
        let expected_source_offset = offset
            .checked_add(item_index)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| RuntimeClientError::ObservationUnavailable {
                message: "auxiliary audit record offset overflow".into(),
            })?;
        if record.audit_offset != expected_source_offset
            || record.audit_offset > PROTOCOL_INTEGER_MAX
            || record.journal_sequence <= previous_journal
            || record.journal_sequence > cut.through
        {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "auxiliary audit records are not on dense captured offsets".into(),
            });
        }
        previous_journal = record.journal_sequence;
        let item = complete_item(&record).map_err(|message| {
            RuntimeClientError::ObservationUnavailable {
                message: message.into(),
            }
        })?;
        items.push(item);
    }
    let total = source_page.total;
    let revision = cut.domain_revisions.get("auxiliary").copied().unwrap_or(0);
    Ok(CheckpointPage {
        collections: vec![CollectionFrontier {
            domain: DOMAIN.into(),
            revision,
            total,
            loaded,
            next_offset: source_page.next_offset,
        }],
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: DOMAIN.into(),
        total,
        items,
        next_offset: source_page.next_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryAuditFact, AuxiliaryAuditRecord, AuxiliaryBusinessSource,
        AuxiliaryCallOutcome, AuxiliaryCallStart, AuxiliaryFingerprint, AuxiliaryInputEvidence,
        AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
        AuxiliaryUsageCompleteness,
    };
    use crate::model::finish::ModelFinishReason;
    use crate::model::snapshot::ModelRequestOwner;
    use crate::model::types::{ModelUsage, UsageDetails};
    use crate::runtime::identity::{
        AuxiliaryCallId, ConversationId, MessageId, RequestId, SystemUuidV7Generator,
    };

    fn fingerprint() -> AuxiliaryFingerprint {
        AuxiliaryFingerprint::new("a".repeat(64)).unwrap()
    }

    fn temporary_call(call_id: AuxiliaryCallId) -> AuxiliaryCallStart {
        AuxiliaryCallStart {
            call_id,
            purpose: AuxiliaryAttribution::new("summary").unwrap(),
            trigger: AuxiliaryAttribution::new("manual").unwrap(),
            business_source: AuxiliaryBusinessSource::Independent,
            input: AuxiliaryInputEvidence::Temporary,
            semantic_input_fingerprint: fingerprint(),
            invocation_fingerprint: fingerprint(),
            selected_message_ids: Vec::new(),
        }
    }

    fn start_call(store: &SqliteConversationStore) -> AuxiliaryCallId {
        let call_id = AuxiliaryCallId::generate();
        store
            .commit_auxiliary_call_start(&temporary_call(call_id.clone()), chrono::Utc::now())
            .unwrap();
        call_id
    }

    fn start_request(store: &SqliteConversationStore, call_id: AuxiliaryCallId) -> RequestId {
        let request_id =
            RequestId::generate_auxiliary(&SystemUuidV7Generator).expect("UUIDv7 request");
        store
            .commit_auxiliary_request_start(
                &AuxiliaryRequestStart {
                    call_id,
                    ordinal: 0,
                    request_id: request_id.clone(),
                    model: "auxiliary-model".into(),
                },
                chrono::Utc::now(),
            )
            .unwrap();
        request_id
    }

    fn primary_message(store: &SqliteConversationStore, ordinal: u64) {
        let message =
            crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
                id: MessageId::new(format!("primary-audit-{ordinal}")),
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: format!("primary history {ordinal}"),
                    },
                )],
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: Some(chrono::Utc::now()),
            });
        store.append_canonical(&message).unwrap();
        let attempt = crate::runtime::identity::AttemptId::new(format!("audit-primary-{ordinal}"));
        for event in [
            crate::events::types::RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
            crate::events::types::RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: crate::model::ModelFinishReason::Stop,
            },
        ] {
            let id = format!(
                "audit-primary-journal-{ordinal}-{}",
                store.auxiliary_audit_cut().unwrap().journal
            );
            let mut fact = envelope(store, &id, event);
            fact.attempt_id = Some(attempt.clone());
            store.append_event(fact).unwrap();
        }
    }

    fn checkpoint_cut(
        source: crate::durable::observation::ObservationReadCut,
    ) -> CheckpointReadCut {
        CheckpointReadCut {
            canonical_foreground: None,
            live_publication: None,
            conversation_id: source.read.conversation_id.clone(),
            activation_id: None,
            revision: 1,
            through: source.read.journal,
            source_cut: Some(source),
            domain_revisions: std::collections::BTreeMap::from([("auxiliary".to_owned(), 1)]),
            collection_cuts: std::collections::BTreeMap::new(),
        }
    }

    fn envelope(
        store: &SqliteConversationStore,
        event_id: &str,
        event: crate::events::types::RuntimeEvent,
    ) -> crate::events::types::RuntimeEventEnvelope {
        crate::events::types::RuntimeEventEnvelope {
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: crate::runtime::identity::EventId::new(event_id),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: chrono::Utc::now(),
            tool_occurrence: None,
            observation: None,
            event,
        }
    }

    fn decoded(item: &CheckpointPageItem) -> AuxiliaryAuditRecord {
        crate::runtime_client::checkpoint::decode_domain_value(
            item.value.clone().expect("complete typed audit item"),
        )
        .unwrap()
    }

    #[test]
    fn auxiliary_audit_pages_use_dense_offsets_and_true_total_across_interleaved_history() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        for ordinal in 0..33 {
            primary_message(&store, ordinal);
            start_call(&store);
        }
        let cut = checkpoint_cut(store.observation_read_cut().unwrap());
        assert!(cut.through > 33, "Journal coordinates are genuinely sparse");

        let first = page(&store, &cut, 0, 32).unwrap();
        assert_eq!(first.total, 33);
        assert_eq!(first.items.len(), 32);
        assert_eq!(first.next_offset, Some(32));
        assert_eq!(first.collections[0].total, 33);
        assert_eq!(first.collections[0].loaded, 32);
        assert_eq!(first.collections[0].next_offset, Some(32));
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.offset)
                .collect::<Vec<_>>(),
            (0..32).collect::<Vec<_>>()
        );
        assert!(first.items.iter().all(|item| {
            item.body_complete == Some(true)
                && !item.deferred
                && item
                    .value
                    .as_ref()
                    .is_some_and(|value| value.get("journal_sequence").is_some())
        }));
        assert!(decoded(&first.items[0]).journal_sequence > 1);

        let last = page(&store, &cut, first.next_offset.unwrap(), 32).unwrap();
        assert_eq!(last.total, 33);
        assert_eq!(last.items.len(), 1);
        assert_eq!(last.items[0].offset, 32);
        assert_eq!(last.next_offset, None);

        let later = start_call(&store);
        assert!(!later.as_str().is_empty());
        let replay = page(&store, &cut, 0, 32).unwrap();
        assert_eq!(replay.items, first.items);
        assert_eq!(replay.total, 33);
    }

    #[test]
    fn empty_audit_after_long_unrelated_history_has_no_phantom_pages() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        for ordinal in 0..64 {
            primary_message(&store, ordinal);
        }
        let cut = checkpoint_cut(store.observation_read_cut().unwrap());
        assert!(cut.through >= 64);
        let page = page(&store, &cut, 0, 32).unwrap();
        assert_eq!(page.total, 0);
        assert!(page.items.is_empty());
        assert_eq!(page.next_offset, None);
        assert_eq!(page.collections[0].loaded, 0);
        assert_eq!(page.collections[0].total, 0);
        assert_eq!(page.collections[0].next_offset, None);
    }

    #[test]
    fn historical_audit_cut_excludes_later_settlement_and_admission() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        primary_message(&store, 0);
        let call_id = start_call(&store);
        let request_id = start_request(&store, call_id.clone());
        let historical = checkpoint_cut(store.observation_read_cut().unwrap());
        let historical_page = page(&store, &historical, 0, 32).unwrap();
        assert_eq!(historical_page.total, 2);
        let AuxiliaryAuditFact::RequestStarted(request) = decoded(&historical_page.items[1]).fact
        else {
            panic!("historical request start")
        };
        assert!(request.request_evidence.is_none());

        let observed_provider_terminal = AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
        };
        let usage = crate::model::types::ModelUsage {
            input_tokens: 17,
            output_tokens: 5,
            total_tokens: 22,
            details: None,
        };
        let request_terminal = envelope(
            &store,
            &format!("auxiliary-request-completed:{request_id}"),
            crate::events::types::RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary {
                    call_id: call_id.clone(),
                    ordinal: 0,
                },
                request_id: request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: Some(usage.clone()),
                generation: None,
            },
        );
        let request_evidence = AuxiliaryRequestEvidence {
            request_id: request_id.clone(),
            observed_provider_terminal: Some(observed_provider_terminal.clone()),
            usage: Some(usage.clone()),
            usage_completeness: AuxiliaryUsageCompleteness::Complete,
            remote_execution_uncertainty: false,
        };
        let call_settled = envelope(
            &store,
            &format!("auxiliary-call-settled:{call_id}"),
            crate::events::types::RuntimeEvent::AuxiliaryCallSettled {
                call_id: call_id.clone(),
                outcome: AuxiliaryCallOutcome::Completed,
                request_evidence: Some(request_evidence),
            },
        );
        store
            .commit_auxiliary_request_settlement(request_terminal, call_settled)
            .unwrap();
        primary_message(&store, 99);
        start_call(&store);

        let retained = page(&store, &historical, 0, 32).unwrap();
        assert_eq!(retained.items, historical_page.items);
        assert_eq!(retained.total, 2);

        let current = checkpoint_cut(store.observation_read_cut().unwrap());
        let current_page = page(&store, &current, 0, 32).unwrap();
        assert_eq!(current_page.total, 5);
        let settled = current_page
            .items
            .iter()
            .map(decoded)
            .find_map(|record| match record.fact {
                AuxiliaryAuditFact::RequestSettled(request) => Some(request),
                _ => None,
            })
            .unwrap();
        let evidence = settled
            .request_evidence
            .expect("paired Call settlement evidence");
        assert_eq!(evidence.request_id, request_id);
        assert_eq!(evidence.usage, Some(usage));
        assert_eq!(
            evidence.observed_provider_terminal,
            Some(observed_provider_terminal)
        );
        assert_eq!(
            evidence.usage_completeness,
            AuxiliaryUsageCompleteness::Complete
        );
    }

    #[test]
    fn exact_u64_usage_survives_store_page_and_structural_decode() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        let call_id = start_call(&store);
        let request_id = start_request(&store, call_id.clone());
        let usage = ModelUsage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            total_tokens: u64::MAX,
            details: Some(UsageDetails {
                reasoning_tokens: Some(u64::MAX),
                cached_input_tokens: Some(u64::MAX),
            }),
        };
        let observed_provider_terminal = AuxiliaryObservedProviderTerminal::Completed {
            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
        };
        let terminal = envelope(
            &store,
            &format!("auxiliary-request-completed:{request_id}"),
            crate::events::types::RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary {
                    call_id: call_id.clone(),
                    ordinal: 0,
                },
                request_id: request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: Some(usage.clone()),
                generation: None,
            },
        );
        let call_settled = envelope(
            &store,
            &format!("auxiliary-call-settled:{call_id}"),
            crate::events::types::RuntimeEvent::AuxiliaryCallSettled {
                call_id: call_id.clone(),
                outcome: AuxiliaryCallOutcome::Completed,
                request_evidence: Some(AuxiliaryRequestEvidence {
                    request_id: request_id.clone(),
                    observed_provider_terminal: Some(observed_provider_terminal),
                    usage: Some(usage.clone()),
                    usage_completeness: AuxiliaryUsageCompleteness::Complete,
                    remote_execution_uncertainty: false,
                }),
            },
        );
        store
            .commit_auxiliary_request_settlement(terminal, call_settled)
            .unwrap();

        let cut = checkpoint_cut(store.observation_read_cut().unwrap());
        let page = page(&store, &cut, 0, 32).unwrap();
        let item = page
            .items
            .iter()
            .find(|item| matches!(decoded(item).fact, AuxiliaryAuditFact::RequestSettled(_)))
            .expect("request settlement audit item");
        let value = item.value.as_ref().expect("complete structural value");
        let wire_usage = &value["fact"]["RequestSettled"]["usage"];
        assert_eq!(wire_usage["input_tokens"], u64::MAX.to_string());
        assert_eq!(wire_usage["output_tokens"], u64::MAX.to_string());
        assert_eq!(wire_usage["total_tokens"], u64::MAX.to_string());
        assert_eq!(
            wire_usage["details"]["reasoning_tokens"],
            u64::MAX.to_string()
        );

        let record = decoded(item);
        let AuxiliaryAuditFact::RequestSettled(request) = record.fact else {
            panic!("request settlement audit fact")
        };
        assert_eq!(request.usage, Some(usage.clone()));
        assert_eq!(
            request
                .request_evidence
                .expect("paired settlement evidence")
                .usage,
            Some(usage)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One end-to-end maximal-encoding contract.
    fn maximal_escaping_identity_and_u64_usage_remain_page_readable() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        let call_id = start_call(&store);
        let request_id = RequestId::new("\u{0001}".repeat(512));
        let model = "\u{0002}".repeat(512);
        assert_eq!(
            request_id.as_str().len(),
            crate::model::auxiliary::AUXILIARY_REQUEST_ID_BYTES_MAX
        );
        assert_eq!(
            model.len(),
            crate::model::auxiliary::AUXILIARY_REQUEST_MODEL_BYTES_MAX
        );
        store
            .commit_auxiliary_request_start(
                &AuxiliaryRequestStart {
                    call_id: call_id.clone(),
                    ordinal: 0,
                    request_id: request_id.clone(),
                    model: model.clone(),
                },
                chrono::Utc::now(),
            )
            .unwrap();

        let usage = ModelUsage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            total_tokens: u64::MAX,
            details: Some(UsageDetails {
                reasoning_tokens: Some(u64::MAX),
                cached_input_tokens: Some(u64::MAX),
            }),
        };
        let terminal = envelope(
            &store,
            &format!("auxiliary-request-completed:{request_id}"),
            crate::events::types::RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary {
                    call_id: call_id.clone(),
                    ordinal: 0,
                },
                request_id: request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: Some(usage.clone()),
                generation: Some(crate::model::generation_evidence::GenerationEvidence {
                    dispatch_after_start_ms: Some(
                        crate::model::auxiliary::AUXILIARY_GENERATION_MS_MAX,
                    ),
                    first_output_ms: Some(crate::model::auxiliary::AUXILIARY_GENERATION_MS_MAX),
                    last_output_ms: Some(crate::model::auxiliary::AUXILIARY_GENERATION_MS_MAX),
                    terminal_ms: crate::model::auxiliary::AUXILIARY_GENERATION_MS_MAX,
                }),
            },
        );
        let settled = envelope(
            &store,
            &format!("auxiliary-call-settled:{call_id}"),
            crate::events::types::RuntimeEvent::AuxiliaryCallSettled {
                call_id: call_id.clone(),
                outcome: AuxiliaryCallOutcome::Completed,
                request_evidence: Some(AuxiliaryRequestEvidence {
                    request_id: request_id.clone(),
                    observed_provider_terminal: Some(
                        AuxiliaryObservedProviderTerminal::Completed {
                            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                        },
                    ),
                    usage: Some(usage.clone()),
                    usage_completeness: AuxiliaryUsageCompleteness::Complete,
                    remote_execution_uncertainty: true,
                }),
            },
        );
        let before = store.auxiliary_audit_cut().unwrap();
        let mut oversized_timing = terminal.clone();
        let crate::events::types::RuntimeEvent::ModelRequestCompleted {
            generation: Some(generation),
            ..
        } = &mut oversized_timing.event
        else {
            unreachable!()
        };
        generation.terminal_ms = u64::MAX;
        assert!(matches!(
            store.commit_auxiliary_request_settlement(oversized_timing, settled.clone()),
            Err(crate::durable::auxiliary::AuxiliaryWriteFailure::KnownNotCommitted { .. })
        ));
        assert_eq!(store.auxiliary_audit_cut().unwrap(), before);
        store
            .commit_auxiliary_request_settlement(terminal, settled)
            .unwrap();

        let cut = checkpoint_cut(store.observation_read_cut().unwrap());
        let page = page(&store, &cut, 0, 32).unwrap();
        let started = page
            .items
            .iter()
            .find(|item| matches!(decoded(item).fact, AuxiliaryAuditFact::RequestStarted(_)))
            .expect("escaped request identity item");
        assert_eq!(
            started.identity,
            format!("auxiliary:request:{request_id}:started")
        );
        let AuxiliaryAuditFact::RequestStarted(request) = decoded(started).fact else {
            panic!("request start audit fact")
        };
        assert_eq!(request.request_id, request_id);
        assert_eq!(request.model, model);

        let settled = page
            .items
            .iter()
            .find(|item| matches!(decoded(item).fact, AuxiliaryAuditFact::RequestSettled(_)))
            .expect("escaped request settlement item");
        assert_eq!(
            settled.identity,
            format!("auxiliary:request:{request_id}:settled")
        );
        let value = settled.value.as_ref().expect("complete settlement value");
        assert_eq!(
            value["fact"]["RequestSettled"]["usage"]["input_tokens"],
            u64::MAX.to_string()
        );
        let AuxiliaryAuditFact::RequestSettled(request) = decoded(settled).fact else {
            panic!("request settlement audit fact")
        };
        assert_eq!(request.model, model);
        assert_eq!(request.usage, Some(usage.clone()));
        assert_eq!(
            request
                .request_evidence
                .expect("paired settlement evidence")
                .usage,
            Some(usage)
        );
    }

    #[test]
    fn complete_item_bound_rejects_non_native_oversized_identity() {
        let record = AuxiliaryAuditRecord {
            audit_offset: 1,
            journal_sequence: 1,
            fact: AuxiliaryAuditFact::RequestStarted(
                crate::model::auxiliary::AuxiliaryRequestAudit {
                    call_id: AuxiliaryCallId::generate(),
                    ordinal: 0,
                    request_id: crate::runtime::identity::RequestId::new("request"),
                    model: "model".into(),
                    started_sequence: 1,
                    terminal_sequence: None,
                    local_terminal: None,
                    normalized_failure_kind: None,
                    generation: None,
                    usage: None,
                    request_evidence: None,
                },
            ),
        };
        assert!(
            crate::runtime_client::checkpoint::bounded_bytes(
                &record,
                crate::model::auxiliary::AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
            )
            .is_some()
        );
        let oversized_identity = format!("auxiliary:request:{}:started", "x".repeat(16_384));
        assert_eq!(
            complete_item_with_identity(&record, oversized_identity).unwrap_err(),
            "auxiliary audit page item exceeds its finite 16 KiB structural bound"
        );
    }

    #[test]
    fn complete_item_preserves_original_record_bound_failure() {
        let record = AuxiliaryAuditRecord {
            audit_offset: 1,
            journal_sequence: 1,
            fact: AuxiliaryAuditFact::RequestStarted(
                crate::model::auxiliary::AuxiliaryRequestAudit {
                    call_id: AuxiliaryCallId::generate(),
                    ordinal: 0,
                    request_id: crate::runtime::identity::RequestId::new("request"),
                    model: "x".repeat(13_000),
                    started_sequence: 1,
                    terminal_sequence: None,
                    local_terminal: None,
                    normalized_failure_kind: None,
                    generation: None,
                    usage: None,
                    request_evidence: None,
                },
            ),
        };
        assert!(
            crate::runtime_client::checkpoint::bounded_bytes(
                &record,
                crate::model::auxiliary::AUXILIARY_AUDIT_RECORD_ENCODED_MAX,
            )
            .is_none()
        );
        assert!(
            crate::runtime_client::checkpoint::bounded_bytes(
                &record,
                crate::model::auxiliary::AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX,
            )
            .is_some()
        );
        assert_eq!(
            complete_item(&record).unwrap_err(),
            "auxiliary audit record exceeds its finite 12 KiB structural bound"
        );
    }

    #[test]
    fn journal_only_page_works_and_mismatched_source_cut_is_rejected() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        start_call(&store);
        let cut = checkpoint_cut(store.observation_read_cut().unwrap());

        let journal_only = CheckpointReadCut {
            source_cut: None,
            ..cut.clone()
        };
        let historical_page = page(&store, &journal_only, 0, 32).unwrap();
        assert_eq!(historical_page.total, 1);
        assert_eq!(historical_page.items[0].offset, 0);
        assert_eq!(historical_page.read_cut, journal_only);

        let mut mismatched = cut;
        mismatched.source_cut.as_mut().unwrap().read.journal = mismatched.through.saturating_sub(1);
        assert!(matches!(
            page(&store, &mismatched, 0, 32),
            Err(RuntimeClientError::ReadCutExpired)
        ));

        let mut wrong_target = journal_only.clone();
        wrong_target.conversation_id = ConversationId::generate();
        assert!(matches!(
            page(&store, &wrong_target, 0, 32),
            Err(RuntimeClientError::ReadCutExpired)
        ));
        for invalid_limit in [0, AUXILIARY_AUDIT_PAGE_LIMIT_MAX + 1] {
            assert!(matches!(
                page(&store, &journal_only, 0, invalid_limit),
                Err(RuntimeClientError::InvalidRequest { .. })
            ));
        }
    }

    #[test]
    fn historical_input_uses_the_full_resource_cut_and_selected_message_refs() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        let message =
            crate::message::types::MessageBlock::User(crate::message::types::UserMessageBlock {
                id: MessageId::new("historical-input-message"),
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: "historical source".into(),
                    },
                )],
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: Some(chrono::Utc::now()),
            });
        store.initialize(std::slice::from_ref(&message)).unwrap();
        let input = store.compaction_resource_cut().unwrap();
        let call_id = AuxiliaryCallId::generate();
        let start = AuxiliaryCallStart {
            call_id,
            purpose: AuxiliaryAttribution::new("title").unwrap(),
            trigger: AuxiliaryAttribution::new("automatic").unwrap(),
            business_source: AuxiliaryBusinessSource::HumanMessage {
                message_id: MessageId::new("historical-input-message"),
            },
            input: AuxiliaryInputEvidence::Historical { cut: input.clone() },
            semantic_input_fingerprint: fingerprint(),
            invocation_fingerprint: fingerprint(),
            selected_message_ids: vec![MessageId::new("historical-input-message")],
        };
        let committed = store
            .commit_auxiliary_call_start(&start, chrono::Utc::now())
            .unwrap();
        assert_eq!(committed.call.input, start.input);
        assert_eq!(committed.call.business_source, start.business_source);
        assert_eq!(
            committed.call.selected_message_ids,
            start.selected_message_ids
        );

        let cut = checkpoint_cut(store.observation_read_cut().unwrap());
        let page = page(&store, &cut, 0, 32).unwrap();
        let AuxiliaryAuditFact::CallStarted(call) = decoded(&page.items[0]).fact else {
            panic!("typed Call audit")
        };
        assert_eq!(call.input, input_evidence(input));
        assert_eq!(call.selected_message_ids, start.selected_message_ids);
    }

    fn input_evidence(cut: crate::context::resources::ResourceReadCut) -> AuxiliaryInputEvidence {
        AuxiliaryInputEvidence::Historical { cut }
    }
}
