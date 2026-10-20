//! Artifact copying preserves the same immutable native history as an ordinary copy.
use super::*;
use crate::durable::inbox::{InboundDraft, LineageSeed};
use crate::durable::reading::{ConversationWindowAt, InheritedTurnOutcome};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::content::FileReference;
use crate::model::finish::ModelFinishReason;
use crate::runtime::identity::{ArtifactId, AttemptId, AuxiliaryCallId, EventId, RequestId};
use crate::tools::ArtifactStore;

mod integrity_tests;

fn complete_turn(store: &SqliteConversationStore, conversation: &ConversationId, index: u32) {
    let attempt = AttemptId::new(format!("native-attempt-{index}"));
    let timestamp = Utc
        .with_ymd_and_hms(2026, 10, 10, 12, 0, index * 3 - 2)
        .unwrap();
    let event = |kind| RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(format!(
            "event-{}",
            store.presentation_frontier().unwrap() + 1
        )),
        sequence: 0,
        conversation_id: conversation.clone(),
        attempt_id: Some(attempt.clone()),
        turn_id: None,
        timestamp,
        event: kind,
    };
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: attempt.clone(),
            turn: TurnId::new(format!("native-step-{index}")),
            retry_number: 0,
        },
        crate::context::resources::LedgerFrontier((store.load_head().unwrap().revision).get()),
        "frozen lineage context".into(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        ModelInvocationConfig {
            model: "historical/model".into(),
            protocol: ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 128,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        4096,
        None,
        false,
        Vec::new(),
        CapabilityRevision::new(1),
        crate::context::assembly::ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    );
    freeze_session_test_input(store, &mut snapshot, &[]);
    store
        .commit_model_turn_start(&[], &snapshot, timestamp)
        .unwrap();
    let mut terminal = event(RuntimeEvent::ModelRequestCompleted {
        owner: crate::model::ModelRequestOwner::Agent {
            identity: snapshot.identity.clone(),
        },
        request_id: snapshot.request_id,
        finish_reason: ModelFinishReason::Stop,
        usage: Some(crate::model::types::ModelUsage {
            input_tokens: u64::from(index) * 32,
            output_tokens: 4,
            total_tokens: u64::from(index) * 32 + 4,
            details: None,
        }),
        generation: None,
    });
    terminal.turn_id = Some(snapshot.identity.turn);
    store.append_event(terminal).unwrap();
    let id = MessageId::new(format!("answer-{index}"));
    store
        .append_canonical_with_event(
            &MessageBlock::Assistant(AssistantMessageBlock {
                id: id.clone(),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: format!("Response {index}"),
                })],
            }),
            event(RuntimeEvent::AssistantMessageCommitted { message_id: id }),
        )
        .unwrap();
    store
        .append_event(event(RuntimeEvent::AttemptCompleted {
            attempt_id: attempt.clone(),
            finish_reason: ModelFinishReason::Stop,
        }))
        .unwrap();
}

pub(super) fn source(
    catalog: &SessionCatalog,
    artifact: bool,
) -> (SqliteConversationStore, LineageReadCut) {
    let (conversation, session, _) = append_history(catalog, &[]);
    let store = store_for(catalog, &session, &conversation);
    let mut content = vec![text("First prompt")];
    if artifact {
        let path = catalog.database_path(&session, &conversation);
        let artifacts = ArtifactStore::new(conversation.clone(), path.parent().unwrap()).unwrap();
        artifacts.put_bounded(b"unused object").unwrap();
        let id = artifacts
            .put_bounded(b"immutable artifact evidence")
            .unwrap();
        assert_eq!(id, ArtifactId::new("artifact_2"));
        content.push(UserContentBlock::File(FileReference {
            artifact_id: id,
            name: Some("evidence.txt".into()),
            mime_type: Some("text/plain".into()),
            description: None,
        }));
    }
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("prompt-1")),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content,
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap(),
            correlation: None,
        })
        .unwrap();
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("start-1"),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: Some(AttemptId::new("native-attempt-1")),
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 1).unwrap(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("native-attempt-1"),
            },
        })
        .unwrap();
    store
        .adopt_pending_batch(accepted.sequence, Some(AttemptId::new("native-attempt-1")))
        .unwrap();
    complete_turn(&store, &conversation, 1);
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "checkpoint",
                    2,
                    "First turn summarized",
                ),
                None,
            ),
        )
        .unwrap();
    let accepted = store
        .accept_inbound(InboundDraft {
            message_id: Some(MessageId::new("prompt-2")),
            source: UserSource::Human,
            kind: InboundKind::Message,
            content: vec![text("Second prompt")],
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 3).unwrap(),
            correlation: None,
        })
        .unwrap();
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("start-2"),
            sequence: 0,
            conversation_id: conversation.clone(),
            attempt_id: Some(AttemptId::new("native-attempt-2")),
            turn_id: None,
            timestamp: Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 4).unwrap(),
            event: RuntimeEvent::AttemptStarted {
                attempt_id: AttemptId::new("native-attempt-2"),
            },
        })
        .unwrap();
    store
        .adopt_pending_batch(accepted.sequence, Some(AttemptId::new("native-attempt-2")))
        .unwrap();
    complete_turn(&store, &conversation, 2);
    let cut = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(cut.turns.len(), 2);
    assert_eq!(cut.completed_responses.len(), 2);
    (store, cut)
}

#[allow(clippy::type_complexity)]
pub(super) fn link_summary_generation(
    store: &SqliteConversationStore,
    checkpoint_id: &str,
) -> (AuxiliaryCallId, RequestId, u64) {
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallStart, AuxiliaryFingerprint,
        AuxiliaryInputEvidence, AuxiliaryRequestStart,
    };
    use crate::runtime::identity::CompactionOperationId;

    let mut candidate = crate::scripted_suites::support::context::checkpoint_fixture(
        store,
        checkpoint_id,
        1,
        "Linked Summary generation",
    );
    let operation_id = CompactionOperationId::new(format!("operation:{checkpoint_id}"));
    let call_id = AuxiliaryCallId::generate();
    let request_id =
        RequestId::generate_auxiliary(&crate::runtime::identity::SystemUuidV7Generator).unwrap();
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: operation_id.clone(),
            },
        })
        .unwrap();
    store
        .commit_auxiliary_call_start(
            &AuxiliaryCallStart {
                call_id: call_id.clone(),
                purpose: crate::model::auxiliary::AuxiliaryAttribution::new("summary").unwrap(),
                trigger: AuxiliaryAttribution::new("automatic").unwrap(),
                business_source: AuxiliaryBusinessSource::Compaction {
                    operation_id: operation_id.clone(),
                },
                input: AuxiliaryInputEvidence::Historical {
                    cut: store.compaction_resource_cut().unwrap(),
                },
                semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
                invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
                selected_message_ids: store
                    .load_canonical()
                    .unwrap()
                    .iter()
                    .take(32)
                    .map(|message| message.id().clone())
                    .collect(),
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
                model: "fixture/summary-model".into(),
            },
            Utc::now(),
        )
        .unwrap();
    super::archive_tests::settle_auxiliary(store, call_id.clone(), request_id.clone(), true);
    let through = store.auxiliary_audit_cut().unwrap().journal;
    candidate.summary_generation = Some(crate::context::checkpoint::SummaryGenerationRef {
        conversation_id: store.conversation_id().clone(),
        call_id: call_id.clone(),
        through_journal: through,
    });
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                store, candidate, None,
            ),
        )
        .unwrap();
    (call_id, request_id, through)
}

/// Build detached historical inspection input from real, cut-bounded audit rows.
/// Open and failed source evidence never authorizes a new local checkpoint.
#[allow(clippy::too_many_lines, clippy::type_complexity)]
pub(super) fn historical_summary_audit_fixture(
    store: &SqliteConversationStore,
    failed: bool,
) -> (AuxiliaryCallId, RequestId, u64, LineageReadCut) {
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallStart, AuxiliaryFingerprint,
        AuxiliaryInputEvidence, AuxiliaryRequestStart,
    };
    use crate::runtime::identity::CompactionOperationId;

    let operation_id = CompactionOperationId::new("operation:summary-open-link");
    let call_id = AuxiliaryCallId::generate();
    let request_id =
        RequestId::generate_auxiliary(&crate::runtime::identity::SystemUuidV7Generator).unwrap();
    store
        .append_event(RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(""),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: None,
            turn_id: None,
            timestamp: Utc::now(),
            event: RuntimeEvent::CompactionStarted {
                operation_id: operation_id.clone(),
            },
        })
        .unwrap();
    store
        .commit_auxiliary_call_start(
            &AuxiliaryCallStart {
                call_id: call_id.clone(),
                purpose: crate::model::auxiliary::AuxiliaryAttribution::new("summary").unwrap(),
                trigger: AuxiliaryAttribution::new("automatic").unwrap(),
                business_source: AuxiliaryBusinessSource::Compaction {
                    operation_id: operation_id.clone(),
                },
                input: AuxiliaryInputEvidence::Historical {
                    cut: store.compaction_resource_cut().unwrap(),
                },
                semantic_input_fingerprint: AuxiliaryFingerprint::new("c".repeat(64)).unwrap(),
                invocation_fingerprint: AuxiliaryFingerprint::new("d".repeat(64)).unwrap(),
                selected_message_ids: store
                    .load_canonical()
                    .unwrap()
                    .iter()
                    .take(32)
                    .map(|message| message.id().clone())
                    .collect(),
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
                model: "fixture/summary-model".into(),
            },
            Utc::now(),
        )
        .unwrap();
    if failed {
        super::archive_tests::settle_auxiliary(store, call_id.clone(), request_id.clone(), false);
    }
    let audit_cut = store.auxiliary_audit_cut().unwrap();
    let through = audit_cut.journal;
    let mut source_cut = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    let checkpoint = source_cut.projections.last_mut().unwrap();
    checkpoint.candidate.summary_generation =
        Some(crate::context::checkpoint::SummaryGenerationRef {
            conversation_id: store.conversation_id().clone(),
            call_id: call_id.clone(),
            through_journal: through,
        });
    checkpoint.fingerprint = checkpoint.candidate.fingerprint();
    let page = store.read_auxiliary_audit(audit_cut, None, 32).unwrap();
    assert!(page.next_offset.is_none());
    assert!(source_cut.auxiliary_audits.is_empty());
    source_cut
        .auxiliary_audits
        .push(crate::durable::reading::AuxiliaryAuditSnapshot {
            origin: crate::durable::reading::AuxiliaryAuditOrigin::Local,
            source_conversation_id: store.conversation_id().clone(),
            call_id: call_id.clone(),
            through_journal: through,
            completeness: if failed {
                crate::durable::reading::AuxiliaryAuditCompleteness::CompleteAtCut
            } else {
                crate::durable::reading::AuxiliaryAuditCompleteness::OpenAtCut
            },
            records: page
                .records
                .into_iter()
                .filter(|record| match &record.fact {
                    crate::model::auxiliary::AuxiliaryAuditFact::CallStarted(call)
                    | crate::model::auxiliary::AuxiliaryAuditFact::CallSettled(call) => {
                        call.call_id == call_id
                    }
                    crate::model::auxiliary::AuxiliaryAuditFact::RequestStarted(request)
                    | crate::model::auxiliary::AuxiliaryAuditFact::RequestSettled(request) => {
                        request.call_id == call_id
                    }
                })
                .collect(),
        });
    assert!(
        store
            .projection_head()
            .unwrap()
            .unwrap()
            .candidate
            .summary_generation
            .is_none()
    );
    (call_id, request_id, through, source_cut)
}

#[test]
#[allow(clippy::too_many_lines)] // Verify real lineage replay, exact payloads and invalid dependency keys.
fn inherited_summary_audits_replay_in_canonical_database_key_order() {
    use std::cmp::Reverse;
    use std::collections::BTreeSet;

    use crate::durable::ConversationStoreError;
    use crate::durable::reading::AuxiliaryAuditSnapshot;

    let (_directory, catalog, _config) = open_catalog();
    let (source_store, _) = source(&catalog, false);
    let (first_call, _, first_through) = link_summary_generation(&source_store, "summary-first");
    let (second_call, _, second_through) = link_summary_generation(&source_store, "summary-second");
    let source_cut = source_store
        .read_lineage_cut(source_store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(source_cut.auxiliary_audits.len(), 2);
    let audit_cut = source_store.auxiliary_audit_cut().unwrap();
    for call_id in [&first_call, &second_call] {
        assert_eq!(
            source_store
                .load_auxiliary_call(call_id, audit_cut)
                .unwrap()
                .unwrap()
                .outcome,
            Some(crate::model::auxiliary::AuxiliaryCallOutcome::Completed)
        );
    }
    let retained: BTreeSet<_> = source_cut
        .projections
        .iter()
        .filter_map(|checkpoint| checkpoint.candidate.summary_generation.as_ref())
        .map(|reference| {
            (
                reference.conversation_id.clone(),
                reference.call_id.clone(),
                reference.through_journal,
            )
        })
        .collect();
    assert_eq!(
        retained,
        BTreeSet::from([
            (
                source_store.conversation_id().clone(),
                first_call,
                first_through,
            ),
            (
                source_store.conversation_id().clone(),
                second_call,
                second_through,
            ),
        ])
    );

    let key = |audit: &AuxiliaryAuditSnapshot| {
        (
            audit.source_conversation_id.as_str().to_owned(),
            audit.call_id.as_str().to_owned(),
            audit.through_journal,
        )
    };
    let mut descending = source_cut.auxiliary_audits.clone();
    descending.sort_by_key(|audit| Reverse(key(audit)));
    assert_eq!(descending.len(), 2);
    assert!(
        key(&descending[0]) > key(&descending[1]),
        "the regression input must be in strictly descending SQLite key order"
    );
    assert_eq!(
        descending.iter().map(key).collect::<BTreeSet<_>>().len(),
        2,
        "the descending regression keys must be unique"
    );

    let target_id = ConversationId::generate();
    let remapped = super::super::lineage_cut(
        &target_id,
        &source_cut,
        None,
        super::super::LineageSide::After,
    )
    .unwrap();
    let seed_with_audits = |audits: Vec<AuxiliaryAuditSnapshot>| {
        LineageSeed::replayed(
            remapped.canonical().to_vec(),
            remapped.surface_history().to_vec(),
        )?
        .with_completed_responses(remapped.completed_responses().to_vec())?
        .with_turns(remapped.turns().to_vec())?
        .with_projections(remapped.projections().to_vec())?
        .with_raw_tool_arguments(remapped.raw_tool_arguments().to_vec())?
        .with_auxiliary_audits(audits)
    };

    let descending_seed = seed_with_audits(descending).unwrap();
    let target = SqliteConversationStore::in_memory(target_id.clone()).unwrap();
    target.initialize_lineage(&descending_seed).unwrap();
    target.initialize_lineage(&descending_seed).unwrap();

    let canonical = descending_seed.auxiliary_audits();
    assert_eq!(canonical.len(), 2);
    assert!(
        key(&canonical[0]) < key(&canonical[1]),
        "the seed constructor must canonicalize dependency order"
    );
    assert_eq!(canonical.iter().map(key).collect::<BTreeSet<_>>().len(), 2);
    for audit in canonical {
        let original = source_cut
            .auxiliary_audits
            .iter()
            .find(|original| key(original) == key(audit))
            .expect("canonicalization preserves every dependency identity");
        assert_eq!(
            audit, original,
            "canonicalization preserves each audit payload exactly"
        );
    }

    let mut ascending_source = source_cut.auxiliary_audits.clone();
    ascending_source.sort_by_key(key);
    let reordered = seed_with_audits(ascending_source).unwrap();
    target.initialize_lineage(&reordered).unwrap();

    let duplicate = canonical[0].clone();
    let duplicate_error =
        seed_with_audits(vec![duplicate.clone(), duplicate]).expect_err("duplicate key must fail");
    assert!(matches!(
        duplicate_error,
        ConversationStoreError::InvalidReference(_)
    ));

    let mut changed_payload = canonical.to_vec();
    for record in &mut changed_payload[0].records {
        if let crate::model::auxiliary::AuxiliaryAuditFact::CallStarted(call)
        | crate::model::auxiliary::AuxiliaryAuditFact::CallSettled(call) = &mut record.fact
        {
            call.trigger = crate::model::auxiliary::AuxiliaryAttribution::new("manual").unwrap();
        }
    }
    let changed_seed =
        seed_with_audits(changed_payload).expect("changed payload remains structurally valid");
    let independent_target = SqliteConversationStore::in_memory(target_id).unwrap();
    independent_target
        .initialize_lineage(&changed_seed)
        .expect("changed payload must be valid for a new target");
    assert_eq!(
        target.initialize_lineage(&changed_seed),
        Err(ConversationStoreError::InitialHistoryMismatch)
    );
}

#[test]
fn summary_checkpoint_overlap_does_not_inherit_auxiliary_execution() {
    let (_directory, catalog, _config) = open_catalog();
    let (store, cut) = source(&catalog, false);
    assert!(!cut.projections.is_empty());
    let (summary_call, summary_request) = super::archive_tests::start_auxiliary(
        &store,
        crate::model::auxiliary::AuxiliaryAttribution::new("summary").unwrap(),
        crate::model::auxiliary::AuxiliaryAttribution::new("automatic").unwrap(),
    );
    super::archive_tests::settle_auxiliary(
        &store,
        summary_call.clone(),
        summary_request.clone(),
        true,
    );
    let clone = catalog.prepare_clone_session(&state(), &cut).unwrap();
    let destination =
        SqliteConversationStore::open_existing(clone.conversation_id.clone(), &clone.database_path)
            .unwrap();
    let inherited = destination
        .read_lineage_cut(destination.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(
        inherited.projections.len(),
        cut.projections.len(),
        "the explicit Summary checkpoint is inherited"
    );
    assert!(destination.read_events(None, 64).unwrap().events.is_empty());
    let cut = destination.auxiliary_audit_cut().unwrap();
    assert!(
        destination
            .read_auxiliary_audit(cut, None, 32)
            .unwrap()
            .records
            .is_empty()
    );
    assert!(
        destination
            .load_auxiliary_call(&summary_call, cut)
            .unwrap()
            .is_none()
    );
    assert!(
        destination
            .load_auxiliary_request(&summary_request, cut)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .load_auxiliary_call(&summary_call, store.auxiliary_audit_cut().unwrap())
            .unwrap()
            .is_some(),
        "history overlap alone does not manufacture a checkpoint dependency or erase source audit"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // Complete clone, fork and deletion lifecycle with independent audit evidence.
fn named_summary_generation_dependency_survives_clone_and_source_deletion() {
    use crate::local_runtime::session::deletion::SessionDeleteResult;

    let (root, mut catalog, _config) = open_catalog();
    let source_session = first_session(&catalog);
    let (store, _) = source(&catalog, false);
    let source_conversation_id = store.conversation_id().clone();
    let (call_id, request_id, through) = link_summary_generation(&store, "summary-link");
    let (unrelated_call, unrelated_request) = super::archive_tests::start_auxiliary(
        &store,
        crate::model::auxiliary::AuxiliaryAttribution::new("btw").unwrap(),
        crate::model::auxiliary::AuxiliaryAttribution::new("manual").unwrap(),
    );
    super::archive_tests::settle_auxiliary(
        &store,
        unrelated_call.clone(),
        unrelated_request.clone(),
        true,
    );
    let (title_call, title_request) = super::archive_tests::start_auxiliary(
        &store,
        crate::model::auxiliary::AuxiliaryAttribution::new("title").unwrap(),
        crate::model::auxiliary::AuxiliaryAttribution::new("automatic").unwrap(),
    );
    super::archive_tests::settle_auxiliary(&store, title_call.clone(), title_request.clone(), true);
    let source_cut = store
        .read_lineage_cut(store.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(source_cut.auxiliary_audits.len(), 1);
    assert_eq!(source_cut.auxiliary_audits[0].call_id, call_id);
    assert_eq!(source_cut.auxiliary_audits[0].through_journal, through);
    assert_eq!(
        source_cut.auxiliary_audits[0].origin,
        crate::durable::reading::AuxiliaryAuditOrigin::Local
    );

    let clone = catalog
        .prepare_clone_session(&state(), &source_cut)
        .unwrap();
    catalog
        .publish_session(&clone, SessionNodeOrigin::New)
        .unwrap();
    let destination = store_for(&catalog, &clone.session_id, &clone.conversation_id);
    let inherited = destination
        .archive_inherited_auxiliary_audit_rows()
        .unwrap();
    assert_eq!(inherited.len(), 4);
    assert!(inherited.iter().all(|record| {
        record["source_conversation_id"] == serde_json::json!(source_conversation_id)
            && record["call_id"] == serde_json::json!(call_id)
            && record["through_journal"] == serde_json::json!(through)
            && record["origin"]["type"] == "inherited"
            && record["completeness"] == "complete_at_cut"
    }));
    let destination_cut = destination.auxiliary_audit_cut().unwrap();
    assert!(
        destination
            .read_auxiliary_audit(destination_cut, None, 32)
            .unwrap()
            .records
            .is_empty()
    );
    assert!(
        destination
            .load_auxiliary_call(&call_id, destination_cut)
            .unwrap()
            .is_none()
    );
    assert!(
        destination
            .load_auxiliary_request(&request_id, destination_cut)
            .unwrap()
            .is_none()
    );
    assert!(
        destination
            .load_auxiliary_call(&unrelated_call, destination_cut)
            .unwrap()
            .is_none()
    );
    assert!(
        destination
            .load_auxiliary_request(&unrelated_request, destination_cut)
            .unwrap()
            .is_none()
    );
    assert!(
        destination
            .load_auxiliary_call(&title_call, destination_cut)
            .unwrap()
            .is_none()
    );
    assert!(
        destination
            .load_auxiliary_request(&title_request, destination_cut)
            .unwrap()
            .is_none()
    );
    assert!(
        destination
            .load_request_owner(&request_id)
            .unwrap()
            .is_none()
    );
    let inherited_cut = destination
        .read_lineage_cut(destination.load_head().unwrap().revision)
        .unwrap();
    assert_eq!(inherited_cut.auxiliary_audits.len(), 1);
    assert_eq!(inherited_cut.auxiliary_audits[0].call_id, call_id);
    assert_eq!(
        inherited_cut.auxiliary_audits[0].source_conversation_id,
        source_conversation_id
    );
    assert_eq!(
        inherited_cut.auxiliary_audits[0].origin,
        crate::durable::reading::AuxiliaryAuditOrigin::Inherited {
            source_conversation_id: source_conversation_id.clone(),
        }
    );

    let repeated = catalog
        .prepare_clone_session(&state(), &inherited_cut)
        .unwrap();
    catalog
        .publish_session(&repeated, SessionNodeOrigin::New)
        .unwrap();
    let repeated_store = store_for(&catalog, &repeated.session_id, &repeated.conversation_id);
    let repeated_rows = repeated_store
        .archive_inherited_auxiliary_audit_rows()
        .unwrap();
    assert_eq!(repeated_rows.len(), 4);
    assert!(repeated_rows.iter().all(|record| {
        record["source_conversation_id"] == serde_json::json!(source_conversation_id)
            && record["call_id"] == serde_json::json!(call_id)
            && record["through_journal"] == serde_json::json!(through)
    }));
    assert!(
        repeated_store
            .load_auxiliary_call(&call_id, repeated_store.auxiliary_audit_cut().unwrap())
            .unwrap()
            .is_none()
    );

    let (fork, editor) = catalog
        .prepare_fork_session(
            &state(),
            &source_cut,
            &MessageId::new("answer-2"),
            crate::local_runtime::session::LineageSide::After,
        )
        .unwrap();
    assert!(editor.is_empty());
    catalog
        .publish_session(&fork, SessionNodeOrigin::New)
        .unwrap();
    let fork_store = store_for(&catalog, &fork.session_id, &fork.conversation_id);
    let fork_rows = fork_store.archive_inherited_auxiliary_audit_rows().unwrap();
    assert_eq!(fork_rows.len(), 4);
    assert!(fork_rows.iter().all(|record| {
        record["source_conversation_id"] == serde_json::json!(source_conversation_id)
            && record["call_id"] == serde_json::json!(call_id)
    }));
    let fork_cut = fork_store.auxiliary_audit_cut().unwrap();
    assert!(
        fork_store
            .load_auxiliary_call(&call_id, fork_cut)
            .unwrap()
            .is_none()
    );

    let (excluded_fork, editor) = catalog
        .prepare_fork_session(
            &state(),
            &source_cut,
            &MessageId::new("prompt-2"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .unwrap();
    assert_eq!(editor, vec![text("Second prompt")]);
    catalog
        .publish_session(&excluded_fork, SessionNodeOrigin::New)
        .unwrap();
    let excluded_store = store_for(
        &catalog,
        &excluded_fork.session_id,
        &excluded_fork.conversation_id,
    );
    assert!(
        excluded_store
            .archive_inherited_auxiliary_audit_rows()
            .unwrap()
            .is_empty()
    );

    drop(destination);
    drop(repeated_store);
    drop(fork_store);
    drop(excluded_store);
    drop(store);
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&source_session) else {
        panic!("source deletion preview");
    };
    let work = catalog
        .commit_delete(&source_session, &preview.target_revision)
        .unwrap()
        .unwrap();
    let cleaned = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, cleaned),
        SessionDeleteResult::Deleted { .. }
    ));
    drop(catalog);

    let reopened = reopen_catalog(root.path());
    let destination = store_for(&reopened, &clone.session_id, &clone.conversation_id);
    let inherited = destination
        .archive_inherited_auxiliary_audit_rows()
        .unwrap();
    assert_eq!(inherited.len(), 4);
    assert!(inherited.iter().all(|record| {
        record["source_conversation_id"] == serde_json::json!(source_conversation_id)
            && record["call_id"] == serde_json::json!(call_id)
    }));
}

#[test]
#[allow(clippy::too_many_lines)]
fn clone_and_fork_do_not_inherit_overlapping_auxiliary_execution_after_source_deletion() {
    use crate::local_runtime::session::deletion::SessionDeleteResult;

    let (root, mut catalog, _config) = open_catalog();
    let source_session = first_session(&catalog);
    let (store, cut) = source(&catalog, false);
    let (btw_call, btw_request) = super::archive_tests::start_auxiliary(
        &store,
        crate::model::auxiliary::AuxiliaryAttribution::new("btw").unwrap(),
        crate::model::auxiliary::AuxiliaryAttribution::new("manual").unwrap(),
    );
    super::archive_tests::settle_auxiliary(&store, btw_call.clone(), btw_request.clone(), true);
    let (classification_call, classification_request) = super::archive_tests::start_auxiliary(
        &store,
        crate::model::auxiliary::AuxiliaryAttribution::new("classification").unwrap(),
        crate::model::auxiliary::AuxiliaryAttribution::new("automatic").unwrap(),
    );
    super::archive_tests::settle_auxiliary(
        &store,
        classification_call.clone(),
        classification_request.clone(),
        true,
    );
    let source_audit_cut = store.auxiliary_audit_cut().unwrap();
    assert_eq!(
        store
            .read_auxiliary_audit(source_audit_cut, None, 32)
            .unwrap()
            .records
            .len(),
        8
    );
    for call_id in [&btw_call, &classification_call] {
        assert_eq!(
            store
                .load_auxiliary_call(call_id, source_audit_cut)
                .unwrap()
                .unwrap()
                .outcome,
            Some(crate::model::auxiliary::AuxiliaryCallOutcome::Completed)
        );
    }

    let clone = catalog.prepare_clone_session(&state(), &cut).unwrap();
    let (fork, editor) = catalog
        .prepare_fork_session(
            &state(),
            &cut,
            &MessageId::new("prompt-2"),
            crate::local_runtime::session::LineageSide::Before,
        )
        .unwrap();
    assert_eq!(editor, vec![text("Second prompt")]);
    catalog
        .publish_session(&clone, SessionNodeOrigin::New)
        .unwrap();
    catalog
        .publish_session(&fork, SessionNodeOrigin::New)
        .unwrap();

    for (destination, expected_turns, expected_responses) in [(&clone, 2, 2), (&fork, 1, 1)] {
        let target = store_for(
            &catalog,
            &destination.session_id,
            &destination.conversation_id,
        );
        let target_cut = target.auxiliary_audit_cut().unwrap();
        assert!(
            target
                .read_auxiliary_audit(target_cut, None, 32)
                .unwrap()
                .records
                .is_empty()
        );
        for call_id in [&btw_call, &classification_call] {
            assert!(
                target
                    .load_auxiliary_call(call_id, target_cut)
                    .unwrap()
                    .is_none()
            );
            assert!(
                store
                    .load_auxiliary_call(call_id, source_audit_cut)
                    .unwrap()
                    .is_some(),
                "absence in the independent destination is not evidence that the source did not execute"
            );
        }
        for request_id in [&btw_request, &classification_request] {
            assert!(
                target
                    .load_auxiliary_request(request_id, target_cut)
                    .unwrap()
                    .is_none()
            );
            assert!(target.load_request_owner(request_id).unwrap().is_none());
        }
        let inherited = target
            .read_lineage_cut(target.load_head().unwrap().revision)
            .unwrap();
        assert!(!inherited.canonical.is_empty());
        assert_eq!(inherited.turns.len(), expected_turns);
        assert_eq!(inherited.completed_responses.len(), expected_responses);
        assert!(target.read_events(None, 64).unwrap().events.is_empty());
    }

    drop(store);
    let SessionDeleteResult::Preview { preview } = catalog.delete_preview(&source_session) else {
        panic!("source deletion preview");
    };
    let work = catalog
        .commit_delete(&source_session, &preview.target_revision)
        .unwrap()
        .unwrap();
    let cleaned = work.run();
    assert!(matches!(
        catalog.finish_delete(&work.record, cleaned),
        SessionDeleteResult::Deleted { .. }
    ));
    drop(catalog);

    let reopened = reopen_catalog(root.path());
    for (destination, expected_turns, expected_responses) in [(&clone, 2, 2), (&fork, 1, 1)] {
        assert!(reopened.snapshot(&destination.session_id).is_ok());
        let target = store_for(
            &reopened,
            &destination.session_id,
            &destination.conversation_id,
        );
        let inherited = target
            .read_lineage_cut(target.load_head().unwrap().revision)
            .unwrap();
        assert!(!inherited.canonical.is_empty());
        assert_eq!(inherited.turns.len(), expected_turns);
        assert_eq!(inherited.completed_responses.len(), expected_responses);
        let target_cut = target.auxiliary_audit_cut().unwrap();
        assert!(
            target
                .read_auxiliary_audit(target_cut, None, 32)
                .unwrap()
                .records
                .is_empty()
        );
        for call_id in [&btw_call, &classification_call] {
            assert!(
                target
                    .load_auxiliary_call(call_id, target_cut)
                    .unwrap()
                    .is_none()
            );
        }
        for request_id in [&btw_request, &classification_request] {
            assert!(
                target
                    .load_auxiliary_request(request_id, target_cut)
                    .unwrap()
                    .is_none()
            );
            assert!(target.load_request_owner(request_id).unwrap().is_none());
        }
    }
    assert!(reopened.snapshot(&source_session).is_err());
}

#[allow(clippy::too_many_lines)] // Verify every seed field and native reading surface before and after reopening.
fn verify_copy(copy: &PreparedLineage, expected: &LineageSeed, artifact: bool, count: usize) {
    for _ in 0..2 {
        // Each iteration closes the SQLite store, then reads only destination authority.
        let store = SqliteConversationStore::open_existing(
            copy.conversation_id.clone(),
            &copy.database_path,
        )
        .unwrap();
        let inherited = store
            .read_lineage_cut(store.load_head().unwrap().revision)
            .unwrap();
        assert_eq!(
            inherited.turns,
            expected.turns(),
            "materialization must persist bootstrap turn_provenance"
        );
        assert_eq!(
            inherited.completed_responses,
            expected.completed_responses()
        );
        assert_eq!(inherited.surface_history, expected.surface_history());
        assert_eq!(inherited.projections.len(), expected.projections().len());
        for (actual, expected) in inherited.projections.iter().zip(expected.projections()) {
            assert_eq!(actual.activation.activation, expected.activation.activation);
            assert_eq!(actual.activation.ledger, expected.activation.ledger);
            assert_eq!(actual.activation.origin, expected.activation.origin);
            assert!(actual.activation.destination_installation.is_some());
            assert_eq!(
                actual.candidate.base_frontier,
                expected.candidate.base_frontier
            );
            assert_eq!(
                actual.candidate.summary_metadata,
                expected.candidate.summary_metadata
            );
        }
        store.projected_history().unwrap();
        let mut canonical = expected.canonical().to_vec();
        if artifact {
            let MessageBlock::User(prompt) = &mut canonical[0] else {
                panic!("human prompt")
            };
            let UserContentBlock::File(reference) = &mut prompt.content[1] else {
                panic!("artifact")
            };
            assert_eq!(reference.artifact_id, ArtifactId::new("artifact_2"));
            reference.artifact_id = ArtifactId::new("artifact_1");
            let owner = ArtifactStore::new(
                copy.conversation_id.clone(),
                copy.database_path.parent().unwrap(),
            )
            .unwrap();
            assert_eq!(
                owner.read_bounded(&reference.artifact_id).unwrap(),
                b"immutable artifact evidence"
            );
            assert_eq!(
                ArtifactStore::archive_lengths(owner.root(), || Ok(()))
                    .unwrap()
                    .len(),
                1
            );
        } else {
            assert!(
                ArtifactStore::archive_lengths(copy.database_path.parent().unwrap(), || Ok(()))
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(inherited.canonical, canonical);
        assert_eq!(
            inherited.messages,
            store
                .load_surface_snapshot(store.load_head().unwrap().revision)
                .unwrap()
        );
        assert!(store.read_events(None, 64).unwrap().events.is_empty());
        assert!(
            store
                .read_request_snapshots(None, 64)
                .unwrap()
                .snapshots
                .is_empty()
        );
        assert_eq!(store.presentation_frontier().unwrap(), 0);
        assert_eq!(
            crate::context::occupancy::read(&store, 0).unwrap(),
            crate::context::occupancy::inherit(
                expected.turns(),
                expected.canonical(),
                expected.projections()
            ),
            "Artifact remapping must preserve the selected inherited occupancy"
        );
        let directory = store.conversation_turns(0, 64).unwrap();
        assert_eq!(directory.total, count);
        assert_eq!(directory.turns.len(), count);
        for (index, turn) in directory.turns.iter().enumerate() {
            assert_eq!(turn.id, expected.turns()[index].id);
            assert_eq!(turn.ordinal, index + 1);
            assert_eq!(
                turn.prompt,
                if index == 0 {
                    "First prompt"
                } else {
                    "Second prompt"
                }
            );
            assert_eq!(turn.response, format!("Response {}", index + 1));
            let selected = store.conversation_turns(index, 1).unwrap();
            assert_eq!(selected.turns, vec![turn.clone()]);
            let window = store
                .conversation_window(
                    &ConversationWindowAt::Turn {
                        id: turn.id.clone(),
                        cut: directory.cut.clone(),
                    },
                    1,
                )
                .unwrap();
            assert_eq!(window.target.as_ref(), Some(&turn.id));
            assert_eq!(window.target_cursor, turn.cursor);
            let mut page =
                crate::runtime_client::snapshot::transcript_page_view(window.page).unwrap();
            crate::runtime_client::response::native_oracle_decorate(&store, &mut page).unwrap();
            let process = page.entries[0].turn_process.as_ref().unwrap();
            assert_eq!(
                process.outcome,
                crate::durable::reading::TurnProcessOutcome::Completed
            );
            assert!(process.event_id.is_none());
            assert_eq!(process.ended_at, expected.turns()[index].ended_at);
        }
    }
}

fn copy_history(fork: bool) {
    let mut comparable = Vec::new();
    for artifact in [false, true] {
        let (_root, mut catalog, _) = open_catalog();
        let (store, cut) = source(&catalog, artifact);
        let (copy, selected) = if fork {
            let (copy, draft) = catalog
                .prepare_fork_session(
                    &state(),
                    &cut,
                    &MessageId::new("prompt-2"),
                    super::super::LineageSide::Before,
                )
                .unwrap();
            assert_eq!(draft, vec![text("Second prompt")]);
            let boundary = store
                .load_user_message_boundaries(cut.surface_revision)
                .unwrap()
                .into_iter()
                .find(|boundary| boundary.message.id == MessageId::new("prompt-2"))
                .unwrap();
            let selected = store
                .read_lineage_cut(SurfaceRevision::new(boundary.surface_revision.get() - 1))
                .unwrap();
            (copy, selected)
        } else {
            (catalog.prepare_clone_session(&state(), &cut).unwrap(), cut)
        };
        let count = if fork { 1 } else { 2 };
        assert_eq!(selected.turns.len(), count);
        let expected = super::super::lineage_cut(
            &copy.conversation_id,
            &selected,
            None,
            super::super::LineageSide::After,
        )
        .unwrap();
        for (original, copied) in selected.turns.iter().zip(expected.turns()) {
            assert_eq!(original.id, copied.id);
            assert_eq!(original.execution, copied.execution);
            assert_eq!(original.started_at, copied.started_at);
            assert_eq!(original.ended_at, copied.ended_at);
            assert_eq!(copied.outcome, InheritedTurnOutcome::Completed);
            assert_ne!(original.process_message_ids, copied.process_message_ids);
            assert_ne!(original.prompt_message_id, copied.prompt_message_id);
        }
        comparable.push((
            expected
                .turns()
                .iter()
                .map(|turn| {
                    (
                        turn.id.attempt_id.clone(),
                        turn.outcome.clone(),
                        turn.execution.clone(),
                        turn.started_at,
                        turn.ended_at,
                    )
                })
                .collect::<Vec<_>>(),
            expected
                .projections()
                .iter()
                .map(|checkpoint| checkpoint.candidate.base_frontier.0)
                .collect::<Vec<_>>(),
        ));
        catalog
            .publish_session(&copy, SessionNodeOrigin::New)
            .unwrap();
        drop(store);
        verify_copy(&copy, &expected, artifact, count);
    }
    assert_eq!(
        comparable[0], comparable[1],
        "Artifact presence cannot change inherited history semantics"
    );
}

#[test]
fn artifact_clone_preserves_complete_native_provenance_after_restart() {
    copy_history(false);
}

#[test]
fn artifact_fork_preserves_only_selected_native_prefix_after_restart() {
    copy_history(true);
}

#[test]
fn artifact_tree_preserves_selected_occupancy_and_native_history_after_restart() {
    for artifact in [false, true] {
        let (_root, catalog, _) = open_catalog();
        let (store, cut) = source(&catalog, artifact);
        let session_id = catalog.document.sessions.keys().next().unwrap().clone();
        // The first cut predates the retained compaction and the newer request.
        // Selecting source latest occupancy would give 64 instead of 32 tokens.
        for count in [1, 2] {
            let selected = if count == 1 {
                store
                    .read_projection_lineage_cut(
                        crate::context::resources::LedgerFrontier(2),
                        crate::context::checkpoint::ProjectionActivationId(0),
                    )
                    .unwrap()
            } else {
                store.read_lineage_cut(cut.surface_revision).unwrap()
            };
            let (copy, draft) = catalog
                .prepare_tree_node(
                    &session_id,
                    &state(),
                    &selected,
                    &MessageId::new(format!("answer-{count}")),
                    super::super::LineageSide::After,
                )
                .unwrap();
            assert!(draft.is_empty());
            let expected = super::super::lineage_cut(
                &copy.conversation_id,
                &selected,
                None,
                super::super::LineageSide::After,
            )
            .unwrap();
            assert_eq!(
                crate::context::occupancy::inherit(
                    expected.turns(),
                    expected.canonical(),
                    expected.projections()
                )
                .unwrap()
                .input_tokens,
                count as u64 * 32
            );
            verify_copy(&copy, &expected, artifact, count);
        }
    }
}
