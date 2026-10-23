//! Request snapshot and model-turn start regressions of the SQLite store.

use crate::context::assembly::ContextGeneration;
use crate::durable::sqlite::schema::shape::*;
use crate::durable::sqlite::schema::*;
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;
use crate::durable::{
    CanonicalStore, InboxStore, JournalStore, ObservationStore, ProjectionStore, RequestStore,
};
use crate::durable::{ConversationStore, ConversationStoreError, ModelTurnStartCommitDisposition};
use crate::events::types::RuntimeEvent;
use crate::events::types::SubagentTerminalState;
use crate::message::content::TextBlock;
use crate::message::types::{
    AgentStatusGenerationMetadata, AgentStatusModuleId, AssistantMessageBlock, ContextKind,
    ToolMessageBlock,
};
use crate::message::types::{
    AssistantContentBlock, ContentBlockIndex, ContributionEmission, InboundKind, MessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::ModelRequestOwner;
use crate::model::finish::ModelFinishReason;
use crate::model::snapshot::RequestIdentity;
use crate::model::snapshot::RequestSnapshot;
use crate::model::types::ModelProtocol;
use crate::model::types::ModelRequest;
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, MessageId, ToolCallId, ToolId, TurnId,
};
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};
use chrono::TimeZone;
use chrono::Utc;
use rusqlite::{Connection, params};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

mod fragment;
mod header_capture;
mod manifest_store;
mod owner;
mod probe;
mod retry;
mod snapshot_validation;
mod span;
mod span_integrity;
mod start_validation;
mod structured_format;

#[test]
fn compaction_resource_memory_bounds_include_all_pages_and_oversized_single_records() {
    use crate::context::resources::{CompactionResources, LedgerFrontier, ResourceLimits};
    let store = store();
    store
        .append_canonical(&user_message("one", "first"))
        .unwrap();
    store
        .append_canonical(&user_message("two", "second"))
        .unwrap();
    let mut resources = CompactionResources::capture(
        &store,
        ResourceLimits {
            page_items: 1,
            total_items: 1,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let first = resources.messages(LedgerFrontier(0)).unwrap();
    assert!(
        resources
            .messages(LedgerFrontier(first.rows[0].0))
            .unwrap_err()
            .to_string()
            .contains("cumulative")
    );
    let mut tiny = CompactionResources::capture(
        &store,
        ResourceLimits {
            record_bytes: 8,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    assert!(
        tiny.messages(LedgerFrontier(0))
            .unwrap_err()
            .to_string()
            .contains("byte bound")
    );
    let mut repeated = CompactionResources::capture(
        &store,
        ResourceLimits {
            total_bytes: 256,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    repeated.originals(&[MessageId::new("one")]).unwrap();
    assert!(
        repeated
            .originals(&[MessageId::new("one")])
            .unwrap_err()
            .to_string()
            .contains("byte bound")
    );
}

#[test]
fn invalid_agent_status_metadata_is_rejected_by_durable_decode() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let conversation_id = ConversationId::new("conv_45cf2e82-15bc-7eb9-8052-70016ae024aa");
    let message_id = MessageId::new("status-invalid-after-persist");
    let status = MessageBlock::User(UserMessageBlock {
        id: message_id.clone(),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "renderer text is irrelevant".to_owned(),
        })],
        source: UserSource::Runtime,
        kind: InboundKind::Context(ContextKind::AgentStatus(
            AgentStatusGenerationMetadata::new(
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                [AgentStatusModuleId::Time, AgentStatusModuleId::Background],
            )
            .expect("valid Agent Status metadata"),
        )),
        timestamp: None,
    });

    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        store.initialize(&[]).unwrap();
        store.append_canonical(&status).unwrap();
        let reloaded = store
            .load_messages(std::slice::from_ref(&message_id))
            .unwrap();
        assert_eq!(reloaded, vec![status.clone()]);
    }

    let mut encoded = serde_json::to_value(&status).unwrap();
    encoded["kind"]["context"]["agent_status"]["modules"] = serde_json::json!([]);
    let encoded = serde_json::to_string(&encoded).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE message_ledger SET message_json=?1 WHERE message_id=?2",
            params![encoded, message_id.as_str()],
        )
        .unwrap();
    drop(connection);

    let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
    let error = reopened
        .load_messages(std::slice::from_ref(&message_id))
        .expect_err("invalid canonical metadata must fail closed");
    assert!(matches!(
        error,
        ConversationStoreError::Storage(message)
            if message.contains("decode Ledger message")
    ));
}

#[test]
fn historical_context_alone_cannot_advance_contribution_receipts() {
    let store = store();
    store.initialize(&[user_message("base", "base")]).unwrap();
    let status = agent_status_message(
        "historical-status",
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    store.append_canonical(&status).unwrap();
    assert_eq!(store.current_logical_step_progress().unwrap(), 0);
    assert!(
        store
            .read_request_snapshots(None, 10)
            .unwrap()
            .snapshots
            .is_empty()
    );
    assert!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .is_none()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn immutable_projection_activations_survive_compactions_growth_and_restart() {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let conversation = ConversationId::generate();
    let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
    let history = vec![
        user_message("a", "A"),
        user_message("b", "B"),
        user_message("c", "C"),
    ];
    store.initialize(&history).unwrap();
    let first =
        checkpoint_commit_fixture(&store, checkpoint_fixture(&store, "P1", 1, "first"), None);
    let receipt1 = store.commit_projection(first.clone()).unwrap();
    store.append_canonical(&user_message("d", "D")).unwrap();
    let second =
        checkpoint_commit_fixture(&store, checkpoint_fixture(&store, "P2", 2, "second"), None);
    let receipt2 = store.commit_projection(second).unwrap();
    let mut replayed = store.commit_projection(first).unwrap();
    assert!(receipt1.newly_committed);
    assert!(!replayed.newly_committed);
    replayed.newly_committed = true;
    assert_eq!(replayed, receipt1);
    assert_eq!(
        store.projection_head().unwrap().unwrap(),
        receipt2.checkpoint
    );
    assert_eq!(store.load_canonical().unwrap().len(), 4);
    drop(store);
    let reopened = SqliteConversationStore::open(conversation, &path).unwrap();
    let timeline = reopened
        .projection_timeline(crate::context::checkpoint::ProjectionActivationId(0), 64)
        .unwrap();
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0], receipt1.checkpoint);
    assert_eq!(timeline[1], receipt2.checkpoint);
    let active = reopened.projected_history().unwrap();
    assert_eq!(
        active
            .messages
            .iter()
            .filter_map(|m| m.canonical_id())
            .cloned()
            .collect::<Vec<_>>(),
        [MessageId::new("c"), MessageId::new("d")]
    );
}

#[test]
fn every_checkpoint_transaction_stage_fault_exposes_only_the_old_complete_state() {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    for fault in [
        CompactionFaultOperation::BeforeContentInsert,
        CompactionFaultOperation::AfterContentInsert,
        CompactionFaultOperation::AfterActivationInsert,
        CompactionFaultOperation::AfterHeadTransition,
        CompactionFaultOperation::BeforeCompletionInsert,
        CompactionFaultOperation::AfterCompletionInsert,
        CompactionFaultOperation::AfterReceiptInsert,
    ] {
        let store = store();
        let history = vec![user_message("a", "A"), user_message("b", "B")];
        store.initialize(&history).unwrap();
        let input =
            checkpoint_commit_fixture(&store, checkpoint_fixture(&store, "P1", 2, "summary"), None);
        let fingerprint = input.candidate.fingerprint();
        store.arm_compaction_fault_script([fault]);
        assert!(store.commit_projection(input.clone()).is_err());
        assert_eq!(store.load_canonical().unwrap(), history);
        assert!(store.projection_head().unwrap().is_none());
        assert!(
            store
                .projection_timeline(crate::context::checkpoint::ProjectionActivationId(0), 64)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .projection_receipt(&input.operation_id, &fingerprint)
                .unwrap()
                .is_none()
        );
        assert!(store.read_events(None, 64).unwrap().events.is_empty());
        let receipt = store.commit_projection(input).unwrap();
        assert_eq!(
            store.projection_head().unwrap().unwrap(),
            receipt.checkpoint
        );
        assert_eq!(store.read_events(None, 64).unwrap().events.len(), 1);
    }
}

/// The frozen request snapshot used by the request-start tests.
#[test]
#[allow(clippy::too_many_lines)] // One native checkpoint/start/recovery proof for all four typed pairings.
fn all_four_projection_tool_pairings_start_exact_requests_without_recovery_execution() {
    use crate::context::checkpoint::{
        CheckpointBlockIndex, CheckpointCallCoordinate, CheckpointItem, CheckpointMessage,
        CheckpointMessageOccurrence, ProjectedCallSource,
    };
    use crate::model::projected::{
        CanonicalMessageRef, DerivedModelMessage, ProjectedMessage, SemanticTransformation,
    };
    let call = crate::tools::types::ToolCall {
        id: ToolCallId::new("wire-call"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".into(),
        arguments: serde_json::json!({}),
    };
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("physical-assistant"),
        content: vec![AssistantContentBlock::ToolCall(call.clone())],
    });
    let result = MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new("physical-result"),
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            assistant.id().clone(),
            ContentBlockIndex::new(0),
        ),
        tool_call_id: call.id,
        tool_id: call.tool_id,
        result: ToolExecutionResult {
            status: ToolExecutionStatus::Success,
            content: vec![crate::tools::types::ToolResultContent::Image(
                crate::message::content::ImageReference {
                    artifact_id: crate::runtime::identity::ArtifactId::new("frozen-image"),
                    alt: Some("source image".into()),
                },
            )],
            duration_ms: 1,
            exit_code: None,
            artifacts: vec![],
            deliveries: vec![],
            truncation: None,
            workflow: None,
            managed_output: None,
        },
    });
    for derived_call in [false, true] {
        for derived_result in [false, true] {
            let store = store();
            store
                .initialize(&[assistant.clone(), result.clone()])
                .unwrap();
            let canonical = store.load_canonical().unwrap();
            let mut candidate = crate::scripted_suites::support::context::checkpoint_fixture(
                &store,
                "typed-pair",
                2,
                "unused",
            );
            candidate.summary_metadata = None;
            candidate.items = [&assistant, &result]
                .into_iter()
                .enumerate()
                .map(|(index, message)| {
                    let derived = if index == 0 {
                        derived_call
                    } else {
                        derived_result
                    };
                    CheckpointItem {
                        message: if derived {
                            CheckpointMessage::Derived(Box::new(DerivedModelMessage {
                                message: ProjectedMessage::from(message),
                                sources: vec![CanonicalMessageRef::of(
                                    store.conversation_id(),
                                    message,
                                )],
                                checkpoint_sources: vec![],
                                transformations: vec![SemanticTransformation::Summary],
                            }))
                        } else {
                            CheckpointMessage::Original(CanonicalMessageRef::of(
                                store.conversation_id(),
                                message,
                            ))
                        },
                        call_sources: if index == 0 && derived {
                            vec![ProjectedCallSource {
                                block: CheckpointBlockIndex(0),
                                assistant: CanonicalMessageRef::of(
                                    store.conversation_id(),
                                    &assistant,
                                ),
                                canonical_block: ContentBlockIndex::new(0),
                            }]
                        } else {
                            vec![]
                        },
                        result_call: (index == 1 && derived).then(|| CheckpointCallCoordinate {
                            message: CheckpointMessageOccurrence {
                                checkpoint_id: candidate.checkpoint_id.clone(),
                                item_index: 0,
                            },
                            block: CheckpointBlockIndex(0),
                        }),
                    }
                })
                .collect();
            store
                .commit_projection(
                    crate::scripted_suites::support::context::checkpoint_commit_fixture(
                        &store, candidate, None,
                    ),
                )
                .unwrap();
            crate::runtime::recovery::recover(&store, &crate::runtime::types::SystemClock).unwrap();
            assert_eq!(
                store.load_canonical().unwrap(),
                canonical,
                "projection never fabricates recovery results"
            );
            assert!(store.unsettled_canonical_tool_calls(1).unwrap().is_empty());
            let history = store.projected_history().unwrap();
            let mut snapshot =
                test_request_snapshot(store.load_head().unwrap().revision, invocation());
            snapshot.history_cut = history.cut.clone();
            let mut request = ModelRequest {
                response_schema: None,
                messages: history.messages.clone(),
                images: crate::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: None,
                continuation_source: None,
            };
            let mut projected = request.messages.to_vec();
            crate::model::images::omit_images(&mut projected);
            for (before, after) in history.messages.iter().zip(&mut projected) {
                if before != after
                    && let Some(source) = before.as_canonical()
                {
                    after.mark_transformed(
                        source,
                        store.conversation_id(),
                        SemanticTransformation::ImageDowngrade,
                    );
                }
            }
            request.messages = projected.into();
            assert!(
                request.messages[1].canonical_id().is_none(),
                "media changes acquire request-local Derived identity"
            );
            snapshot
                .freeze_input(store.conversation_id(), &request)
                .unwrap();
            snapshot.tool_associations = crate::model::occurrence::freeze_associations(
                &history,
                &snapshot.request_id,
                &request,
                None,
            )
            .unwrap();
            store
                .commit_model_turn_start(&[], &snapshot, Utc::now())
                .unwrap();
            assert_eq!(
                store
                    .reconstruct_model_request(&snapshot.request_id)
                    .unwrap(),
                request
            );
            assert!(
                store
                    .read_events(None, 64)
                    .unwrap()
                    .events
                    .iter()
                    .all(|e| !matches!(
                        e.event,
                        RuntimeEvent::ToolExecutionStarted { .. }
                            | RuntimeEvent::ToolExecutionCompleted { .. }
                    ))
            );
        }
    }
}

#[test]
fn request_start_is_atomic_and_reconstructs_from_durable_history() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let conversation_id = ConversationId::new("conv_cf107b20-dc7b-70e4-83e3-eea701e0da84");
    let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
    let a = user_message("a", "A");
    let b = user_message("b", "B");
    store.initialize(&[a.clone(), b.clone()]).unwrap();
    let revision = store.load_head().unwrap().revision;
    let invocation = invocation();
    let mut snapshot = test_request_snapshot(revision, invocation.clone());
    let expected = ModelRequest {
        response_schema: None,
        continuation_source: None,
        images: crate::model::images::ResolvedImages::new(),
        invocation: invocation.clone(),
        messages: crate::model::input::canonical_input(&[a.clone(), b.clone()]).into(),
        tools: vec![],
        effective_system_prompt: snapshot.effective_system_prompt.clone(),
        continuation: None,
    };
    snapshot.freeze_input(&conversation_id, &expected).unwrap();
    store.arm_request_start_fault_script([RequestStartFaultOperation::BeforeContextAppend]);
    assert!(
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .is_err()
    );
    assert!(
        store
            .read_request_snapshots(None, 32)
            .unwrap()
            .snapshots
            .is_empty()
    );
    assert!(store.read_events(None, 20).unwrap().events.is_empty());
    store.arm_fail_event_times(1);
    assert!(
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .is_err()
    );
    assert!(
        store
            .read_request_snapshots(None, 32)
            .unwrap()
            .snapshots
            .is_empty()
    );

    let started = store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    assert_eq!(started.started.sequence, 1);
    assert!(matches!(
        started.started.event,
        RuntimeEvent::ModelRequestStarted { ref request_id, .. } if request_id == &snapshot.request_id
    ));
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        expected
    );

    store
        .append_canonical(&user_message("later", "later"))
        .unwrap();
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        expected
    );
    drop(store);
    let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
    assert_eq!(
        reopened
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        expected
    );
    assert_eq!(
        reopened
            .read_request_snapshots(None, 32)
            .unwrap()
            .snapshots
            .len(),
        1
    );
    assert_eq!(
        reopened
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .unwrap()
            .started
            .sequence,
        1
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One atomicity/replay scenario and its complete durable observations.
fn contribution_emission_is_atomic_with_start_and_exactly_replayed() {
    let store = store();
    let base = user_message("base", "base");
    store.initialize(std::slice::from_ref(&base)).unwrap();
    let revision = store.load_head().unwrap().revision.next();
    let message_id = MessageId::new("status-todo-1");
    let context = agent_status_message(
        message_id.as_str(),
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    let emission = ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "todo-fingerprint-1".to_owned(),
    };
    let mut snapshot = status_start_snapshot(revision, message_id.clone(), vec![emission.clone()]);
    let timestamp = Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap();
    freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));

    let started = store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, timestamp)
        .expect("status and start commit together");
    assert_eq!(started.started.sequence, 1);
    assert_eq!(
        started.disposition,
        ModelTurnStartCommitDisposition::NewlyCommitted
    );
    let events = store.read_events(None, 10).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(
        started.contribution_emissions,
        vec![events[1].clone()],
        "the typed receipt owns the exact emission fact after the start fact"
    );
    assert_eq!(
        started.events().map(Clone::clone).collect::<Vec<_>>(),
        events,
        "the receipt exposes the durable sequence in order"
    );
    match &events[1].event {
        RuntimeEvent::ContextContributionEmitted {
            request_id,
            message_id: stored_message_id,
            emission: stored_emission,
            logical_step_origin,
            ..
        } => {
            assert_eq!(request_id, &snapshot.request_id);
            assert_eq!(stored_message_id, &message_id);
            assert_eq!(stored_emission, &emission);
            assert_eq!(*logical_step_origin, 1);
        }
        other => panic!("expected contribution emission fact, got {other:?}"),
    }
    let head = store
        .latest_contribution_emission(
            &crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::AgentStatus,
            ),
            "active_actionable",
        )
        .unwrap()
        .expect("durable latest-emission head");
    assert_eq!(
        head.producer,
        crate::runtime::identity::ContextContributorIdentity::Native(
            crate::runtime::identity::NativeContextContributor::AgentStatus
        )
    );
    assert_eq!(head.key, emission.key);
    assert_eq!(head.fingerprint, emission.fingerprint);
    assert_eq!(head.request_id, snapshot.request_id);
    assert_eq!(head.canonical_message_id, message_id);
    assert_eq!(head.event_sequence, events[1].sequence);

    let retried = store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, timestamp)
        .expect("exact start replay is idempotent");
    assert_eq!(retried.started, started.started);
    assert_eq!(
        retried.contribution_emissions,
        started.contribution_emissions
    );
    assert_eq!(
        retried.disposition,
        ModelTurnStartCommitDisposition::IdempotentReplay
    );
    assert_eq!(store.read_events(None, 10).unwrap().events.len(), 2);

    let mut contradictory = snapshot.clone();
    contradictory
        .contributions
        .first_mut()
        .expect("contribution")
        .emissions[0]
        .fingerprint = "contradictory-fingerprint".to_owned();
    assert!(matches!(
        store.commit_model_turn_start(std::slice::from_ref(&context), &contradictory, timestamp,),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(store.read_events(None, 10).unwrap().events.len(), 2);
    assert_eq!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .expect("head remains committed")
            .fingerprint,
        emission.fingerprint
    );
}

/// The Todo cooldown origin is assigned inside the same transaction that
/// installs request-scoped context and the canonical Agent Status. Two
/// context appends therefore produce one semantic progress unit, and the
/// committed reminder starts with zero elapsed progress of its own.
#[test]
fn todo_origin_is_assigned_at_the_post_context_start_boundary() {
    let store = store();
    let base = user_message("base", "base");
    store.initialize(std::slice::from_ref(&base)).unwrap();
    let base_revision = store.load_head().unwrap().revision;
    let runtime_context = request_context_message("runtime-observation", "tool observation");
    let status_message_id = MessageId::new("status-origin");
    let status = agent_status_message(
        status_message_id.as_str(),
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    let emission = ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "origin-fingerprint".to_owned(),
    };
    let mut snapshot = status_start_snapshot_for(
        "origin-attempt",
        "1",
        base_revision.next().next(),
        status_message_id.clone(),
        vec![emission.clone()],
        vec![runtime_context.id().clone(), status_message_id.clone()],
    );
    freeze_test_input(
        &store,
        &mut snapshot,
        &[runtime_context.clone(), status.clone()],
    );

    let started = store
        .commit_model_turn_start(
            &[runtime_context, status],
            &snapshot,
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
        )
        .expect("same-start RuntimeToolObservation and Agent Status commit");

    assert_eq!(store.current_logical_step_progress().unwrap(), 1);
    let head = store
        .latest_contribution_emission(
            &crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::AgentStatus,
            ),
            "active_actionable",
        )
        .unwrap()
        .expect("latest Todo head");
    assert_eq!(head.logical_step_origin, 1);
    assert_eq!(
        head.logical_step_origin,
        store.current_logical_step_progress().unwrap()
    );
    assert_eq!(head.canonical_message_id, status_message_id);
    assert_eq!(
        head.event_sequence,
        started.contribution_emissions[0].sequence
    );
    assert!(matches!(
        started.contribution_emissions[0].event,
        RuntimeEvent::ContextContributionEmitted {
            logical_step_origin: 1,
            emission: ref committed,
            ..
        } if committed == &emission
    ));

    // An exact replay verifies the same store-assigned origin and does
    // not advance the sequence a second time.
    let replay = store
        .commit_model_turn_start(
            &[
                request_context_message("runtime-observation", "tool observation"),
                agent_status_message(
                    "status-origin",
                    Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
                    &[AgentStatusModuleId::Todo],
                ),
            ],
            &snapshot,
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
        )
        .expect("exact start replay");
    assert_eq!(
        replay.disposition,
        ModelTurnStartCommitDisposition::IdempotentReplay
    );
    assert_eq!(
        replay.contribution_emissions,
        started.contribution_emissions
    );
    assert_eq!(store.current_logical_step_progress().unwrap(), 1);
}

/// Todo's four-unit window is measured from the committed origin, not
/// from the status preparation snapshot. Later fresh primary starts add
/// exactly one unit each; an overflow retry, a status-free retry, and
/// request-scoped context do not create another unit.
#[test]
fn logical_step_progress_counts_later_fresh_starts_once_and_overflow_retries_zero() {
    let store = store();
    let base = user_message("base", "base");
    store.initialize(std::slice::from_ref(&base)).unwrap();
    let status_message_id = MessageId::new("status-progress");
    let emission = ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "progress-fingerprint".to_owned(),
    };
    let mut first = status_start_snapshot(
        store.load_head().unwrap().revision.next(),
        status_message_id.clone(),
        vec![emission],
    );
    let status = agent_status_message(
        status_message_id.as_str(),
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    freeze_test_input(&store, &mut first, std::slice::from_ref(&status));
    store
        .commit_model_turn_start(
            std::slice::from_ref(&status),
            &first,
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
        )
        .unwrap();
    let origin = store
        .latest_contribution_emission(
            &crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::AgentStatus,
            ),
            "active_actionable",
        )
        .unwrap()
        .expect("first Todo head")
        .logical_step_origin;
    assert_eq!(origin, 1);

    // The overflow request belongs to the already-started first logical
    // step. It is a real start snapshot, but it cannot consume cooldown
    // progress or move the emission origin.
    let retry_revision = store.load_head().unwrap().revision;
    let mut overflow_retry =
        progress_request_snapshot("attempt-1", "1", 1, retry_revision, Vec::new());
    freeze_test_input(&store, &mut overflow_retry, &[]);
    store
        .commit_model_turn_start(&[], &overflow_retry, Utc::now())
        .expect("overflow retry start");
    assert_eq!(store.current_logical_step_progress().unwrap(), origin);

    for index in 0..3 {
        let revision = store.load_head().unwrap().revision;
        let mut snapshot = progress_request_snapshot(
            &format!("later-attempt-{index}"),
            "1",
            0,
            revision,
            Vec::new(),
        );
        freeze_test_input(&store, &mut snapshot, &[]);
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .expect("later fresh primary start");
    }
    assert_eq!(
        store.current_logical_step_progress().unwrap() - origin,
        3,
        "three later fresh starts are strictly before the inclusive boundary"
    );
    assert_eq!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .expect("Todo head remains the first emission")
            .logical_step_origin,
        origin
    );

    let fourth_revision = store.load_head().unwrap().revision;
    let mut fourth =
        progress_request_snapshot("later-attempt-3", "1", 0, fourth_revision, Vec::new());
    freeze_test_input(&store, &mut fourth, &[]);
    store
        .commit_model_turn_start(&[], &fourth, Utc::now())
        .expect("fourth later fresh primary start");
    assert_eq!(store.current_logical_step_progress().unwrap() - origin, 4);
    assert_eq!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .expect("Todo head remains unchanged without a status")
            .logical_step_origin,
        origin
    );
}

/// Typed projection compaction retires model-visible history, but it is not Todo
/// progress. In particular, retiring the Agent Status message does not
/// reset or consume the durable reminder window.
#[test]
fn compaction_does_not_reset_or_advance_logical_step_progress() {
    let store = store();
    let base = user_message("base", "base");
    store.initialize(std::slice::from_ref(&base)).unwrap();
    let status_id = MessageId::new("status-before-compaction");
    let status = agent_status_message(
        status_id.as_str(),
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    let emission = ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "compaction-fingerprint".to_owned(),
    };
    let mut snapshot = status_start_snapshot(
        store.load_head().unwrap().revision.next(),
        status_id.clone(),
        vec![emission],
    );
    freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&status));
    store
        .commit_model_turn_start(
            std::slice::from_ref(&status),
            &snapshot,
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
        )
        .unwrap();
    let origin = store
        .latest_contribution_emission(
            &crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::AgentStatus,
            ),
            "active_actionable",
        )
        .unwrap()
        .expect("Todo head")
        .logical_step_origin;
    assert_eq!(origin, 1);

    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "summary-after-status",
                    2,
                    "summary",
                ),
                None,
            ),
        )
        .expect("compaction commits");

    assert_eq!(store.current_logical_step_progress().unwrap(), origin);
    assert_eq!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .expect("durable Todo head survives compaction")
            .logical_step_origin,
        origin
    );
}

/// Time and Background are optional presentation modules in one status
/// generation. Their canonical status membership is not another logical
/// primary start, so enabling either one cannot move Todo's cadence.
#[test]
fn optional_status_modules_do_not_add_logical_step_progress_units() {
    let store = store();
    let base = user_message("base", "base");
    store.initialize(std::slice::from_ref(&base)).unwrap();
    let runtime_context = request_context_message("runtime-context", "observation");
    let status_id = MessageId::new("status-all-modules");
    let status = agent_status_message(
        status_id.as_str(),
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[
            AgentStatusModuleId::Time,
            AgentStatusModuleId::Background,
            AgentStatusModuleId::Todo,
        ],
    );
    let mut snapshot = status_start_snapshot_for(
        "all-modules-attempt",
        "1",
        store.load_head().unwrap().revision.next().next(),
        status_id.clone(),
        vec![ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "all-modules-fingerprint".to_owned(),
        }],
        vec![runtime_context.id().clone(), status_id],
    );
    snapshot.contributions.last_mut().unwrap().metadata =
        ContextKind::AgentStatus(status.agent_status_metadata().unwrap().clone());
    freeze_test_input(
        &store,
        &mut snapshot,
        &[runtime_context.clone(), status.clone()],
    );
    store
        .commit_model_turn_start(
            &[runtime_context, status],
            &snapshot,
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 3, 0).unwrap(),
        )
        .unwrap();

    assert_eq!(
        store.current_logical_step_progress().unwrap(),
        1,
        "all optional status sections and same-start context share one start unit"
    );
    assert_eq!(
        store
            .latest_contribution_emission(
                &crate::runtime::identity::ContextContributorIdentity::Native(
                    crate::runtime::identity::NativeContextContributor::AgentStatus
                ),
                "active_actionable"
            )
            .unwrap()
            .expect("Todo head")
            .logical_step_origin,
        1
    );
}

#[test]
fn agent_status_start_faults_roll_back_message_emission_and_head_together() {
    for fault in [
        RequestStartFaultOperation::AfterContributionEventInsert,
        RequestStartFaultOperation::AfterContributionHeadUpsert,
    ] {
        let store = store();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let message_id = MessageId::new(format!("status-fault-{fault:?}"));
        let context = agent_status_message(
            message_id.as_str(),
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        let emission = ContributionEmission {
            key: "active_actionable".to_owned(),
            fingerprint: "fault-fingerprint".to_owned(),
        };
        let snapshot = status_start_snapshot(
            store.load_head().unwrap().revision.next(),
            message_id,
            vec![emission],
        );
        store.arm_request_start_fault_script([fault]);
        assert!(
            store
                .commit_model_turn_start(
                    std::slice::from_ref(&context),
                    &snapshot,
                    Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
                )
                .is_err()
        );

        assert_eq!(
            store.load_head().unwrap().revision,
            crate::conversation::SurfaceRevision::new(1)
        );
        assert_eq!(store.load_canonical().unwrap(), vec![base]);
        assert!(
            store
                .read_request_snapshots(None, 10)
                .unwrap()
                .snapshots
                .is_empty()
        );
        assert!(store.read_events(None, 10).unwrap().events.is_empty());
        assert_eq!(
            store.current_logical_step_progress().unwrap(),
            0,
            "a failed status start does not advance Todo progress"
        );
        assert!(
            store
                .latest_contribution_emission(
                    &crate::runtime::identity::ContextContributorIdentity::Native(
                        crate::runtime::identity::NativeContextContributor::AgentStatus
                    ),
                    "active_actionable"
                )
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn agent_status_start_binds_metadata_to_the_exact_status_message() {
    let store = store();
    let base = user_message("base", "base");
    store.initialize(std::slice::from_ref(&base)).unwrap();
    let emission = ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "fingerprint".to_owned(),
    };
    let expected_id = MessageId::new("expected-status");
    let actual = agent_status_message(
        "actual-status",
        Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
        &[AgentStatusModuleId::Todo],
    );
    let snapshot = status_start_snapshot(
        store.load_head().unwrap().revision.next(),
        expected_id,
        vec![emission],
    );
    assert!(matches!(
        store.commit_model_turn_start(
            std::slice::from_ref(&actual),
            &snapshot,
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
        ),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert!(store.read_events(None, 10).unwrap().events.is_empty());
    assert_eq!(store.load_canonical().unwrap(), vec![base]);
}

#[test]
fn contribution_head_resume_uses_one_bounded_lookup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let conversation_id = ConversationId::new("conv_f9113f99-5f0f-7b59-8f83-495e254f63f1");
    let emission = ContributionEmission {
        key: "active_actionable".to_owned(),
        fingerprint: "resume-fingerprint".to_owned(),
    };
    {
        let store = SqliteConversationStore::open(conversation_id.clone(), &path).unwrap();
        let base = user_message("base", "base");
        store.initialize(std::slice::from_ref(&base)).unwrap();
        let message_id = MessageId::new("status-resume");
        let context = agent_status_message(
            message_id.as_str(),
            Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap(),
            &[AgentStatusModuleId::Todo],
        );
        let mut snapshot = status_start_snapshot(
            store.load_head().unwrap().revision.next(),
            message_id,
            vec![emission.clone()],
        );
        freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));
        store
            .commit_model_turn_start(
                std::slice::from_ref(&context),
                &snapshot,
                Utc.with_ymd_and_hms(2026, 8, 27, 6, 1, 0).unwrap(),
            )
            .unwrap();
    }
    let reopened = SqliteConversationStore::open(conversation_id, &path).unwrap();
    let found = reopened
        .latest_contribution_emission(
            &crate::runtime::identity::ContextContributorIdentity::Native(
                crate::runtime::identity::NativeContextContributor::AgentStatus,
            ),
            &emission.key,
        )
        .unwrap()
        .expect("committed suppression survives reopen");
    assert_eq!(found.fingerprint, emission.fingerprint);
    assert_eq!(found.logical_step_origin, 1);
    assert_eq!(reopened.current_logical_step_progress().unwrap(), 1);
    assert_eq!(reopened.contribution_head_lookup_reads(), 1);
}

#[test]
fn stale_projection_head_rejects_request_start_without_snapshot_or_context_admission() {
    use crate::scripted_suites::support::context::{checkpoint_commit_fixture, checkpoint_fixture};
    let root = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        ConversationId::generate(),
        &root.path().join("start.sqlite"),
    )
    .unwrap();
    store.initialize(&[user_message("a", "original")]).unwrap();
    store
        .commit_projection(checkpoint_commit_fixture(
            &store,
            checkpoint_fixture(&store, "P1", 1, "first"),
            None,
        ))
        .unwrap();
    let context = request_context_message("ctx-1", "request-scoped context");
    let mut snapshot = context_start_snapshot(
        store.load_head().unwrap().revision.next(),
        vec![MessageId::new("ctx-1")],
    );
    freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));
    store
        .commit_projection(checkpoint_commit_fixture(
            &store,
            checkpoint_fixture(&store, "P2", 1, "second"),
            None,
        ))
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE projection_head SET activation=1", [])
        .unwrap();
    let before = store.load_canonical().unwrap();
    let events = store.read_events(None, 64).unwrap().events;
    let error = store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
        .unwrap_err();
    assert!(error.to_string().contains("Projection Head"), "{error}");
    assert_eq!(store.load_canonical().unwrap(), before);
    let after = store.read_events(None, 64).unwrap().events;
    assert_eq!(after, events);
    assert_eq!(
        after
            .iter()
            .filter(|e| matches!(e.event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        0
    );
    let snapshots: u64 = store
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM request_snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(snapshots, 0);
    // Correcting only the injected damage proves the snapshot/context fixture
    // is otherwise valid: the obsolete input is rejected, while freezing
    // current history lets this actual-request identity start exactly once.
    store
        .lock()
        .unwrap()
        .execute("UPDATE projection_head SET activation=2", [])
        .unwrap();
    store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
        .unwrap_err();
    let mut current = snapshot;
    freeze_test_input(&store, &mut current, std::slice::from_ref(&context));
    store
        .commit_model_turn_start(std::slice::from_ref(&context), &current, Utc::now())
        .unwrap();
    assert_eq!(
        store
            .read_events(None, 64)
            .unwrap()
            .events
            .iter()
            .filter(|e| matches!(e.event, RuntimeEvent::ModelRequestStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        store
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM request_snapshots", [], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
}

/// Issue #12 (M9b): the request-scoped context, the Request Snapshot,
/// and the `ModelRequestStarted` fact commit in one transaction; the
/// snapshot's Surface revision is the head the context appends created.
#[test]
fn model_turn_start_commits_context_snapshot_and_event_atomically() {
    let store = store();
    let a = user_message("a", "A");
    store.initialize(std::slice::from_ref(&a)).unwrap();
    let base_revision = store.load_head().unwrap().revision;
    let context = request_context_message("ctx-1", "request-scoped context");
    let mut snapshot = context_start_snapshot(base_revision.next(), vec![MessageId::new("ctx-1")]);
    snapshot
        .freeze_input(
            &store.conversation_id,
            &ModelRequest {
                response_schema: None,
                continuation_source: None,
                images: crate::model::images::ResolvedImages::new(),
                invocation: snapshot.invocation.clone(),
                messages: crate::model::input::canonical_input(&[a.clone(), context.clone()])
                    .into(),
                tools: snapshot.tool_definitions.clone(),
                effective_system_prompt: snapshot.effective_system_prompt.clone(),
                continuation: snapshot.continuation.clone(),
            },
        )
        .unwrap();
    let started = store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
        .expect("start commits");
    assert!(matches!(
        started.started.event,
        RuntimeEvent::ModelRequestStarted { ref request_id, .. } if request_id == &snapshot.request_id
    ));
    let head = store.load_head().unwrap();
    assert_eq!(head.revision, base_revision.next());
    assert_eq!(head.active_message_ids.len(), 2);
    assert_eq!(store.load_canonical().unwrap().len(), 2);
    // The snapshot's revision resolves to the post-context surface.
    let reconstructed = store
        .reconstruct_model_request(&snapshot.request_id)
        .expect("reconstruct");
    assert_eq!(
        reconstructed.messages.to_vec(),
        crate::model::input::canonical_input(&[a.clone(), context.clone()])
    );
    // An exact retry is idempotent and returns the original start fact.
    let retried = store
        .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
        .expect("exact retry");
    assert_eq!(retried.started.sequence, started.started.sequence);
    // A retry whose context differs from the committed facts fails.
    let different = request_context_message("ctx-1", "different content");
    assert!(matches!(
        store.commit_model_turn_start(std::slice::from_ref(&different), &snapshot, Utc::now()),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    // A retry missing a committed context message fails.
    let missing = request_context_message("ctx-2", "never committed");
    assert!(
        store
            .commit_model_turn_start(std::slice::from_ref(&missing), &snapshot, Utc::now())
            .is_err()
    );
}

/// Issue #12 (M9b): the durable request-start authority binds the exact
/// ordered request-scoped context of a request. An idempotent retry must
/// prove exact ordered equality — an empty retry, a prefix, a reorder, an
/// extra message, and a same-ids/different-body retry all fail — so the
/// retried start can never substitute a different context set.
#[test]
fn model_turn_start_retry_enforces_exact_ordered_context_equality() {
    let store = store();
    let a = user_message("a", "A");
    store.initialize(std::slice::from_ref(&a)).unwrap();
    let base_revision = store.load_head().unwrap().revision;
    let ctx1 = request_context_message("ctx-1", "first context");
    let ctx2 = request_context_message("ctx-2", "second context");
    let mut snapshot = context_start_snapshot(
        base_revision.next().next(),
        vec![MessageId::new("ctx-1"), MessageId::new("ctx-2")],
    );
    freeze_test_input(&store, &mut snapshot, &[ctx1.clone(), ctx2.clone()]);
    let started = store
        .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &snapshot, Utc::now())
        .expect("start commits");

    // The exact ordered retry is idempotent and returns the original fact.
    let retried = store
        .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &snapshot, Utc::now())
        .expect("exact ordered retry");
    assert_eq!(retried.started.sequence, started.started.sequence);

    // Empty retry fails: the complete ordered set is required.
    assert!(
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .is_err()
    );
    // Prefix retry fails.
    assert!(
        store
            .commit_model_turn_start(std::slice::from_ref(&ctx1), &snapshot, Utc::now())
            .is_err()
    );
    // Reordered retry fails.
    assert!(
        store
            .commit_model_turn_start(&[ctx2.clone(), ctx1.clone()], &snapshot, Utc::now())
            .is_err()
    );
    // Extra message fails.
    let extra = request_context_message("ctx-3", "extra");
    assert!(
        store
            .commit_model_turn_start(&[ctx1.clone(), ctx2.clone(), extra], &snapshot, Utc::now())
            .is_err()
    );
    // Same ids with a different body fails.
    let different = request_context_message("ctx-1", "changed body");
    assert!(
        store
            .commit_model_turn_start(&[different, ctx2], &snapshot, Utc::now())
            .is_err()
    );
}

/// Issue #12 (M9b): the fresh model-turn start validates the exact
/// ordered request-scoped context against the snapshot's frozen
/// `request_context_ids` BEFORE any durable mutation. The one input
/// validation rule is shared with the retry path, so an invalid first
/// commit can never append context while persisting a snapshot whose
/// `request_context_ids` disagrees with what it just appended.
///
/// The frozen authority is the ordered `MessageId` set: the snapshot
/// does not duplicate request-scoped message bodies, so on the fresh
/// path the supplied bodies are the truth being committed (there is no
/// prior fact to disagree with). Same-ids/different-body is therefore a
/// retry-path invariant, proved by
/// `model_turn_start_retry_enforces_exact_ordered_context_equality`.
#[test]
fn model_turn_start_fresh_commit_rejects_context_snapshot_mismatch() {
    let a = user_message("a", "A");
    let ctx1 = request_context_message("ctx-1", "first context");
    let ctx2 = request_context_message("ctx-2", "second context");

    // A fresh start carrying the exact ordered context commits.
    let committed = store();
    committed.initialize(std::slice::from_ref(&a)).unwrap();
    let base_revision = committed.load_head().unwrap().revision;
    let mut exact = context_start_snapshot(
        base_revision.next().next(),
        vec![MessageId::new("ctx-1"), MessageId::new("ctx-2")],
    );
    freeze_test_input(&committed, &mut exact, &[ctx1.clone(), ctx2.clone()]);
    committed
        .commit_model_turn_start(&[ctx1.clone(), ctx2.clone()], &exact, Utc::now())
        .expect("the exact ordered fresh start commits");

    // Every mismatched ordered id set is rejected up front: the Surface
    // never advances, the canonical Ledger never gains the context, and
    // no RequestSnapshot or ModelRequestStarted fact is committed.
    let mismatches: Vec<(Vec<MessageBlock>, Vec<MessageId>)> = vec![
        (vec![ctx1.clone(), ctx2.clone()], vec![]),
        (
            vec![ctx1.clone(), ctx2.clone()],
            vec![MessageId::new("ctx-1")],
        ),
        (
            vec![ctx1.clone(), ctx2.clone()],
            vec![MessageId::new("ctx-2"), MessageId::new("ctx-1")],
        ),
        (
            vec![ctx1.clone(), ctx2.clone()],
            vec![
                MessageId::new("ctx-1"),
                MessageId::new("ctx-2"),
                MessageId::new("ctx-3"),
            ],
        ),
    ];
    for (supplied, frozen_ids) in mismatches {
        let store = store();
        store.initialize(std::slice::from_ref(&a)).unwrap();
        let revision = store.load_head().unwrap().revision;
        let snapshot = context_start_snapshot(revision.next(), frozen_ids);
        assert!(
            matches!(
                store.commit_model_turn_start(&supplied, &snapshot, Utc::now()),
                Err(ConversationStoreError::InvalidReference(_))
            ),
            "a fresh start whose supplied context differs from its frozen snapshot must be rejected"
        );
        assert_eq!(
            store.load_head().unwrap().revision,
            revision,
            "the Surface never advanced"
        );
        assert_eq!(
            store.load_canonical().unwrap().len(),
            1,
            "the canonical Ledger never gained the context"
        );
        assert!(
            store
                .read_request_snapshots(None, 32)
                .unwrap()
                .snapshots
                .is_empty(),
            "no RequestSnapshot was committed"
        );
        assert!(
            store.read_events(None, 32).unwrap().events.is_empty(),
            "no ModelRequestStarted fact was committed"
        );
    }
}

/// Issue #12 (M9b): a failure at any internal stage of the start
/// transaction rolls back the request-scoped context, the snapshot, and
/// the start fact together.
#[test]
fn model_turn_start_fault_at_each_stage_rolls_back_everything() {
    for fault in [
        RequestStartFaultOperation::BeforeContextAppend,
        RequestStartFaultOperation::AfterContextAppend,
        RequestStartFaultOperation::AfterSnapshotInsert,
        RequestStartFaultOperation::AfterCompletionInsert,
    ] {
        let store = store();
        let a = user_message("a", "A");
        store.initialize(std::slice::from_ref(&a)).unwrap();
        let base_revision = store.load_head().unwrap().revision;
        let context = request_context_message("ctx-1", "request-scoped context");
        let mut snapshot =
            context_start_snapshot(base_revision.next(), vec![MessageId::new("ctx-1")]);
        store.arm_request_start_fault_script([fault]);
        freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&context));
        assert!(
            store
                .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
                .is_err(),
            "{fault:?} fails the commit"
        );
        let head = store.load_head().unwrap();
        assert_eq!(
            head.revision, base_revision,
            "{fault:?}: the Surface never advanced"
        );
        assert_eq!(
            store.load_canonical().unwrap().len(),
            1,
            "{fault:?}: the request-scoped context never became canonical"
        );
        assert!(
            store
                .read_request_snapshots(None, 32)
                .unwrap()
                .snapshots
                .is_empty(),
            "{fault:?}: no snapshot exists"
        );
        assert!(
            store.read_events(None, 32).unwrap().events.is_empty(),
            "{fault:?}: no start fact exists"
        );
        // The store recovers: the identical start commits cleanly.
        store
            .commit_model_turn_start(std::slice::from_ref(&context), &snapshot, Utc::now())
            .expect("the identical start commits after the injected failure");
    }
}

#[test]
fn request_snapshot_history_is_bounded_and_cursor_paged() {
    let store = store();
    let message = user_message("request-page-message", "request page");
    store.initialize(std::slice::from_ref(&message)).unwrap();
    let revision = store.load_head().unwrap().revision;

    for index in 0..7_u64 {
        let mut snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: AttemptId::new(format!("request-page-attempt-{index}")),
                turn: TurnId::new("1"),
                retry_number: 0,
            },
            crate::context::resources::LedgerFrontier((revision).get()),
            "frozen".to_owned(),
            Vec::new(),
            crate::runtime::RuntimeResourceRevision::new(1),
            invocation(),
            1024,
            None,
            false,
            Vec::new(),
            crate::runtime::identity::CapabilityRevision::new(1),
            ContextGeneration {
                id: index,
                contributors: Vec::new(),
            },
            None,
            Vec::new(),
        );
        freeze_test_input(&store, &mut snapshot, &[]);
        store
            .commit_model_turn_start(&[], &snapshot, Utc::now())
            .expect("persist request snapshot");
    }

    let empty = store.read_request_snapshots(None, 0).expect("empty page");
    assert!(empty.snapshots.is_empty());
    assert_eq!(empty.next_sequence, None);

    let page_one = store.read_request_snapshots(None, 3).expect("first page");
    let page_two = store
        .read_request_snapshots(page_one.next_sequence, 3)
        .expect("second page");
    let page_three = store
        .read_request_snapshots(page_two.next_sequence, 3)
        .expect("third page");
    let page_four = store
        .read_request_snapshots(page_three.next_sequence, 3)
        .expect("terminal empty page");

    assert_eq!(page_one.snapshots.len(), 3);
    assert_eq!(page_two.snapshots.len(), 3);
    assert_eq!(page_three.snapshots.len(), 1);
    assert_eq!(page_one.next_sequence, Some(3));
    assert_eq!(page_two.next_sequence, Some(6));
    assert_eq!(page_three.next_sequence, Some(7));
    assert!(page_four.snapshots.is_empty());
    assert_eq!(page_four.next_sequence, None);

    let pages = [page_one, page_two, page_three];
    let mut ids = Vec::new();
    for page in pages {
        for snapshot in page.snapshots {
            assert!(
                !ids.contains(&snapshot.request_id),
                "cursor pages must not repeat a Request Snapshot"
            );
            ids.push(snapshot.request_id);
        }
    }
    assert_eq!(ids.len(), 7);
    assert_eq!(
        store
            .load_request_snapshot(&ids[4])
            .expect("keyed snapshot lookup")
            .request_id,
        ids[4]
    );
}

#[test]
fn workflow_terminal_rejects_normal_agent_ownership_without_partial_facts() {
    let (store, terminal, output) = workflow_terminal_fixture(
        &crate::runtime::workflow::test_instance("review_pr", "review"),
        true,
    );
    let before = store.read_events(None, 100).unwrap().events;
    assert!(matches!(
        store.commit_workflow_agent_terminal(terminal.clone(), output),
        Err(ConversationStoreError::InvalidReference(_))
    ));
    assert_eq!(store.read_events(None, 100).unwrap().events, before);
    let RuntimeEvent::SubagentTerminalSettled {
        subagent_id,
        child_agent_id,
        ..
    } = terminal.event
    else {
        unreachable!()
    };
    let (notice, report, event) =
        success_publication(&store, &subagent_id, &child_agent_id, "ordinary success");
    store
        .accept_subagent_terminal(Some(notice), report, event)
        .expect("ordinary Agent remains publishable");
    assert_eq!(
        store.read_events(None, 100).unwrap().events.len(),
        before.len() + 1
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn subagent_terminal_provenance_is_authorized_by_ownership_fact() {
    let store = store();
    let conversation_id = store.conversation_id().clone();

    let own = |ordinal: u64, child_agent_id: &AgentId| {
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
        own_normal_subagent(&store, &subagent_id, child_agent_id);
    };
    let publish = |ordinal: u64,
                   claimed_child: &AgentId,
                   authored_by: &AgentId,
                   suffix: &str|
     -> Result<(), ConversationStoreError> {
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
        let (notice, report, event) = success_publication(
            &store,
            &subagent_id,
            claimed_child,
            &format!("terminal-{suffix}"),
        );
        let mut report = report;
        report.source = UserSource::Agent {
            agent_id: authored_by.clone(),
        };
        store
            .accept_subagent_terminal(Some(notice), report, event)
            .map(|_| ())
    };

    let child_a = AgentId::new("agent-a");
    let child_b = AgentId::new("agent-b");

    // A terminal for a child with no durable ownership fact is an
    // invalid reference: the deterministic ownership-identity lookup
    // finds no opening fact.
    assert!(matches!(
        publish(50, &child_a, &child_a, "missing-ownership"),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    // The event's repeated child identity is not authority: S owns A,
    // so a success claiming B and authored by B is rejected.
    own(10, &child_a);
    assert!(matches!(
        publish(10, &child_b, &child_b, "wrong-child"),
        Err(ConversationStoreError::InvalidReference(_))
    ));

    // Correct child provenance succeeds.
    own(11, &child_a);
    publish(11, &child_a, &child_a, "success").expect("correct Agent(A) success");

    // Success must not be Runtime-authored.
    own(12, &child_a);
    {
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 12);
        let (notice, mut report, event) =
            success_publication(&store, &subagent_id, &child_a, "terminal-runtime-success");
        report.source = UserSource::Runtime;
        assert!(matches!(
            store.accept_subagent_terminal(Some(notice), report, event),
            Err(ConversationStoreError::InvalidReference(_))
        ));
    }

    // A failed/cancelled/interrupted terminal must not be Agent-authored.
    own(13, &child_a);
    {
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, 13);
        let (mut report, event) = non_success_publication(
            &store,
            &subagent_id,
            &child_a,
            SubagentTerminalState::Failed,
            "subagent failed",
        );
        report.source = UserSource::Agent {
            agent_id: child_a.clone(),
        };
        assert!(matches!(
            store.accept_subagent_terminal(None, report, event),
            Err(ConversationStoreError::InvalidReference(_))
        ));
    }

    for (ordinal, state) in [
        (14, SubagentTerminalState::Failed),
        (15, SubagentTerminalState::Cancelled),
        (16, SubagentTerminalState::Interrupted),
    ] {
        own(ordinal, &child_a);
        let subagent_id =
            crate::runtime::identity::SubagentId::for_conversation(&conversation_id, ordinal);
        let (report, event) =
            non_success_publication(&store, &subagent_id, &child_a, state, "runtime terminal");
        store
            .accept_subagent_terminal(None, report, event)
            .expect("Runtime-authored terminal");
    }
}

#[test]
fn corrupt_frozen_request_revision_metadata_fails_closed() {
    let request_store = store();
    let message = user_message("request-message", "request body");
    request_store
        .initialize(std::slice::from_ref(&message))
        .unwrap();
    let mut snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("request-corruption-attempt"),
            turn: TurnId::new("1"),
            retry_number: 0,
        },
        crate::context::resources::LedgerFrontier(
            (request_store.load_head().unwrap().revision).get(),
        ),
        "frozen".to_owned(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        invocation(),
        1024,
        None,
        false,
        Vec::new(),
        crate::runtime::identity::CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    );
    freeze_test_input(&request_store, &mut snapshot, &[]);
    request_store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    {
        let connection = request_store.conn.lock().unwrap();
        let json: String = connection
            .query_row(
                "SELECT snapshot_json FROM request_snapshots WHERE request_id=?1",
                [snapshot.request_id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["canonical_frontier"] = serde_json::json!(999_u64);
        connection
            .execute(
                "UPDATE request_snapshots SET canonical_frontier=999,snapshot_json=?1 WHERE request_id=?2",
                params![value.to_string(), snapshot.request_id.as_str()],
            )
            .unwrap();
    }
    assert!(matches!(
        request_store.reconstruct_model_request(&snapshot.request_id),
        Err(ConversationStoreError::InvalidReference(detail))
            if detail.contains("manifest integrity mismatch")
    ));
}

#[test]
#[allow(clippy::too_many_lines)] // One retained-input loss, independent current execution and real SQLite restart proof.
fn incomplete_historical_snapshot_does_not_block_independently_complete_current_checkpoint_or_restart()
 {
    use crate::context::resources::LedgerFrontier;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot-closure.sqlite");
    let id = ConversationId::generate();
    let store = SqliteConversationStore::open(id.clone(), &path).unwrap();
    let reasoning = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("old-native"),
        content: vec![AssistantContentBlock::Reasoning(
            crate::message::types::ReasoningBlock {
                text: None,
                provider_state: Some(
                    crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                        crate::runtime::continuation::OpenAiResponsesContinuation::Stateless {
                            items: vec![
                                serde_json::json!({"type":"reasoning","id":"rs-old","encrypted_content":"retained-native-bytes"}),
                            ],
                        },
                    ),
                ),
            },
        )],
    });
    store
        .initialize(&[user_message("old-user", "historical input"), reasoning])
        .unwrap();
    let mut first = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    first.invocation.protocol = ModelProtocol::OpenAiResponses;
    first.invocation.compat.responses_storage =
        crate::model::catalog::ResponsesStorageMode::Stateless;
    freeze_test_input(&store, &mut first, &[]);
    store
        .commit_model_turn_start(&[], &first, Utc::now())
        .unwrap();
    let mut completed = envelope(
        store.conversation_id(),
        "old-request-completed",
        Some(first.identity.attempt_id.clone()),
        RuntimeEvent::ModelRequestCompleted {
            owner: ModelRequestOwner::Agent {
                identity: first.identity.clone(),
            },
            request_id: first.request_id.clone(),
            finish_reason: ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    );
    completed.turn_id = Some(first.identity.turn.clone());
    store.append_event(completed).unwrap();
    let exact_first = store.reconstruct_model_request(&first.request_id).unwrap();
    assert_eq!(
        exact_first.semantic_input_fingerprint(),
        first.semantic_input_fingerprint
    );
    store
        .commit_projection(
            crate::scripted_suites::support::context::checkpoint_commit_fixture(
                &store,
                crate::scripted_suites::support::context::checkpoint_fixture(
                    &store,
                    "current-checkpoint",
                    2,
                    "independently complete current summary",
                ),
                None,
            ),
        )
        .unwrap();
    {
        let connection = store.lock().unwrap();
        // Deliberately corrupt retained original authority, bypassing the
        // derived catalog FK that ordinarily prevents this deletion.
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        connection
            .execute(
                "DELETE FROM request_snapshots WHERE request_id=?1",
                [first.request_id.as_str()],
            )
            .unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    }
    assert!(
        matches!(
            store.reconstruct_model_request(&first.request_id),
            Err(ConversationStoreError::RequestNotFound(_))
        ),
        "missing retained input never claims exact reconstruction"
    );
    store
        .append_canonical(&user_message("new-user", "fresh current input"))
        .unwrap();
    let mut next = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    next.identity.turn = TurnId::new("next");
    next.request_id = next.identity.request_id();
    next.provisional_message_id = next.identity.provisional_message_id();
    freeze_test_input(&store, &mut next, &[]);
    assert_eq!(next.history_cut.base_frontier, LedgerFrontier(2));
    store
        .commit_model_turn_start(&[], &next, Utc::now())
        .unwrap();
    let before_restart = store.reconstruct_model_request(&next.request_id).unwrap();
    drop(store);
    let reopened = SqliteConversationStore::open_existing(id, &path).unwrap();
    assert_eq!(
        reopened
            .reconstruct_model_request(&next.request_id)
            .unwrap(),
        before_restart
    );
    assert!(reopened.projected_history().is_ok());
}

#[test]
fn missing_frozen_original_ledger_dependency_fails_closed() {
    let missing_message_store = store();
    let missing_message = user_message("missing-request-message", "request body");
    missing_message_store
        .initialize(std::slice::from_ref(&missing_message))
        .unwrap();
    let mut missing_snapshot = RequestSnapshot::new(
        RequestIdentity {
            attempt_id: AttemptId::new("missing-message-attempt"),
            turn: TurnId::new("1"),
            retry_number: 0,
        },
        crate::context::resources::LedgerFrontier(
            (missing_message_store.load_head().unwrap().revision).get(),
        ),
        "frozen".to_owned(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        invocation(),
        1024,
        None,
        false,
        Vec::new(),
        crate::runtime::identity::CapabilityRevision::new(1),
        ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    );
    missing_snapshot
        .freeze_input(
            &missing_message_store.conversation_id,
            &ModelRequest {
                response_schema: None,
                continuation_source: None,
                images: crate::model::images::ResolvedImages::new(),
                invocation: missing_snapshot.invocation.clone(),
                messages: crate::model::input::canonical_input(std::slice::from_ref(
                    &missing_message,
                ))
                .into(),
                tools: vec![],
                effective_system_prompt: missing_snapshot.effective_system_prompt.clone(),
                continuation: None,
            },
        )
        .unwrap();
    missing_message_store
        .commit_model_turn_start(&[], &missing_snapshot, Utc::now())
        .unwrap();
    {
        let connection = missing_message_store.conn.lock().unwrap();
        let missing_message_id = crate::conversation::message_id_of(&missing_message);
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        connection
            .execute(
                "DELETE FROM message_ledger WHERE message_id=?1",
                [missing_message_id.as_str()],
            )
            .unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    }
    assert!(matches!(
        missing_message_store.reconstruct_model_request(&missing_snapshot.request_id),
        Err(ConversationStoreError::InvalidReference(detail))
            if detail.contains("message missing-request-message is unavailable")
    ));
}

#[test]
fn new_schema_birth_rolls_back_late_ddl_and_commits_once_with_wal_normal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("schema-birth.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    configure_connection(&mut connection, false).unwrap();
    let (conflict, _) = PRESENTATION_INDEXES[0];
    connection
        .execute_batch(&format!(
            "CREATE TABLE schema_birth_sentinel(value TEXT); INSERT INTO schema_birth_sentinel VALUES('original'); CREATE INDEX {conflict} ON schema_birth_sentinel(value);"
        ))
        .unwrap();
    assert!(create_schema(&mut connection).is_err());
    assert!(connection.is_autocommit());
    let objects = connection
        .prepare("SELECT name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut original = vec![conflict.to_owned(), "schema_birth_sentinel".to_owned()];
    original.sort();
    assert_eq!(
        objects, original,
        "no partially born schema survives late DDL failure"
    );
    let sentinel: String = connection
        .query_row("SELECT value FROM schema_birth_sentinel", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sentinel, "original");
    connection
        .execute_batch(&format!(
            "DROP INDEX {conflict}; DROP TABLE schema_birth_sentinel;"
        ))
        .unwrap();
    let commits = Arc::new(AtomicUsize::new(0));
    let observed = commits.clone();
    connection
        .commit_hook(Some(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            false
        }))
        .unwrap();
    create_schema(&mut connection).unwrap();
    assert_eq!(
        commits.load(Ordering::SeqCst),
        1,
        "all schema DDL and validation share one commit"
    );
    verify_schema_shape(&connection).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA synchronous", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    let conversation = ConversationId::new("conv_00000000-0000-7000-8000-000000000467");
    bind_identity(&mut connection, &conversation).unwrap();
    assert_eq!(
        commits.load(Ordering::SeqCst),
        2,
        "identity retains its separate native binding transaction"
    );
    drop(connection);
    let reopened = SqliteConversationStore::open_existing(conversation.clone(), &path).unwrap();
    assert_eq!(reopened.conversation_id, conversation);
}

#[test]
fn exact_frozen_request_needs_no_surface_history_replay() {
    let store = store();
    let seed = user_message("seed", "retained exact input");
    store.initialize(std::slice::from_ref(&seed)).unwrap();
    let mut snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    let request = ModelRequest {
        response_schema: None,
        continuation_source: None,
        images: crate::model::images::ResolvedImages::new(),
        invocation: snapshot.invocation.clone(),
        messages: crate::model::input::canonical_input(&[seed]).into(),
        tools: vec![],
        effective_system_prompt: snapshot.effective_system_prompt.clone(),
        continuation: None,
    };
    snapshot
        .freeze_input(&store.conversation_id, &request)
        .unwrap();
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute("DELETE FROM surface_ops", [])
        .unwrap();
    assert!(
        store
            .reconstruct_surface(crate::conversation::SurfaceRevision::new(
                snapshot.canonical_frontier.0
            ))
            .is_err()
    );
    assert_eq!(
        store
            .reconstruct_model_request(&snapshot.request_id)
            .unwrap(),
        request
    );
}

#[test]
fn oversized_frozen_record_is_rejected_before_body_decode() {
    let store = store();
    store.initialize(&[]).unwrap();
    let snapshot = test_request_snapshot(store.load_head().unwrap().revision, invocation());
    store
        .commit_model_turn_start(&[], &snapshot, Utc::now())
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_snapshots SET snapshot_json=CAST(zeroblob(67108865) AS TEXT)",
            [],
        )
        .unwrap();
    assert!(
        matches!(store.load_request_snapshot(&snapshot.request_id), Err(ConversationStoreError::InvalidReference(detail)) if detail.contains("record exceeds the byte budget"))
    );
}

#[test]
fn t19_t20_journal_readiness_is_exact_committed_and_recovered_on_reinstall() {
    struct Observer {
        store: std::sync::Weak<SqliteConversationStore>,
        ready: Mutex<Vec<(u64, u64)>>,
    }
    impl crate::durable::presentation::JournalObserver for Observer {
        fn committed(&self, _through: Option<u64>) {}
        fn ready(&self, through: u64) {
            let frontier = self
                .store
                .upgrade()
                .unwrap()
                .observation_ready_frontier(0, through)
                .unwrap();
            self.ready.lock().unwrap().push((through, frontier));
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("readiness.sqlite");
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936178");
    let store = Arc::new(SqliteConversationStore::open(id.clone(), &path).unwrap());
    let observer = Arc::new(Observer {
        store: Arc::downgrade(&store),
        ready: Mutex::new(Vec::new()),
    });
    assert_eq!(store.observe_journal(observer.clone()).unwrap(), 0);
    for index in 1..=3 {
        let mut event = envelope(
            &id,
            &format!("ready-{index}"),
            Some(AttemptId::new("attempt")),
            RuntimeEvent::TurnStarted,
        );
        event.turn_id = Some(TurnId::new(index.to_string()));
        assert_eq!(store.append_event(event).unwrap().sequence, index);
    }
    assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 0);
    // Owner two publishes first. This neither releases owner one nor skips it.
    store.mark_observation_direct(2, 2).unwrap();
    assert!(
        store
            .observation_event_witness(2, 3)
            .unwrap()
            .unwrap()
            .native_direct
    );
    assert!(
        !store
            .observation_event_witness(1, 3)
            .unwrap()
            .unwrap()
            .native_direct
    );
    assert_eq!(*observer.ready.lock().unwrap(), vec![(2, 0)]);
    assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 0);
    store.mark_observation_ready(1, 1).unwrap();
    assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 2);
    store.mark_observation_ready(1, 1).unwrap();
    assert!(store.mark_observation_ready(3, 4).is_err());
    assert!(store.mark_observation_ready(0, 1).is_err());
    assert_eq!(store.observation_ready_frontier(2, 3).unwrap(), 2);
    // A rolled-back tiny layout update is invisible and creates no wake.
    let wakes = observer.ready.lock().unwrap().len();
    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        observation_readiness::mark(&transaction, 3, 3).unwrap();
        transaction.rollback().unwrap();
    }
    assert_eq!(observer.ready.lock().unwrap().len(), wakes);
    assert_eq!(store.observation_ready_frontier(2, 3).unwrap(), 2);
    let connection = store.lock().unwrap();
    let plan: String = connection.query_row("EXPLAIN QUERY PLAN SELECT MIN(sequence) FROM event_observation_readiness WHERE ready=0 AND sequence>0 AND sequence<=3", [], |row| row.get(3)).unwrap();
    assert!(
        plan.contains("event_observation_readiness_holes_idx"),
        "{plan}"
    );
    drop(connection);
    drop(store);
    let reopened = Arc::new(SqliteConversationStore::open(id, &path).unwrap());
    assert_eq!(reopened.observation_ready_frontier(0, 3).unwrap(), 2);
    let next = Arc::new(Observer {
        store: Arc::downgrade(&reopened),
        ready: Mutex::new(Vec::new()),
    });
    assert_eq!(reopened.observe_journal(next).unwrap(), 3);
    assert_eq!(reopened.observation_ready_frontier(0, 3).unwrap(), 3);
    assert!(
        !reopened
            .observation_event_witness(2, 3)
            .unwrap()
            .unwrap()
            .native_direct
    );
}

#[test]
fn t14_t19_t20_native_receipts_capture_only_released_predecessor_prefix() {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936187");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    store.initialize(&[]).unwrap();
    let make = |name: &str| {
        let mut event = envelope(
            &id,
            name,
            Some(AttemptId::new("prefix-proof")),
            RuntimeEvent::TurnStarted,
        );
        event.turn_id = Some(TurnId::new("1"));
        event
    };
    let first = store.append_event(make("first")).unwrap();
    assert_eq!(first.observation.as_ref().unwrap().ready_before, Some(0));
    store.mark_observation_ready(1, 1).unwrap();
    let held = store.append_event(make("held")).unwrap();
    assert_eq!(held.observation.as_ref().unwrap().ready_before, Some(1));
    let later = store.append_event(make("later")).unwrap();
    assert_eq!(later.observation.as_ref().unwrap().ready_before, Some(1));
    let canonical = store
        .append_canonical(&user_message("prefix-canonical", "canonical"))
        .unwrap();
    assert_eq!(canonical.observation.ready_through, Some(1));
    assert_eq!(canonical.observation.read_cut.read.journal, 3);
    store.mark_observation_ready(3, 3).unwrap();
    assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 1);
    store.mark_observation_ready(2, 2).unwrap();
    assert_eq!(store.observation_ready_frontier(0, 3).unwrap(), 3);
    assert_eq!(
        later.observation.as_ref().unwrap().ready_before,
        Some(1),
        "later publication cannot retag frozen append evidence"
    );
    let follow = store
        .append_canonical(&user_message("prefix-follow", "follow"))
        .unwrap();
    assert_eq!(follow.observation.ready_through, Some(3));
}

#[test]
fn t10_t14_t19_native_single_receipt_preserves_large_other_reason_and_exact_cut() {
    let id = ConversationId::new("conv_2be184c1-ab11-725b-82c3-81daea936185");
    let store = SqliteConversationStore::in_memory(id.clone()).unwrap();
    let attempt = AttemptId::new("native-receipt-reason");
    let started = store
        .append_event(envelope(
            &id,
            "start",
            Some(attempt.clone()),
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
        ))
        .unwrap();
    let source = started
        .observation
        .as_ref()
        .expect("source-owned Single receipt");
    assert_eq!(source.read_cut.read.journal, 1);
    assert_eq!(source.journal_range, Some((1, 1)));
    let reason = "雪😀".repeat(300_000);
    let settled = store
        .append_event(envelope(
            &id,
            "settled",
            Some(attempt.clone()),
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: ModelFinishReason::Other {
                    reason: reason.clone(),
                },
            },
        ))
        .unwrap();
    let receipt = settled
        .observation
        .as_ref()
        .expect("native terminal receipt");
    let crate::durable::observation::NativeObservationTransition::Single(witness) =
        &receipt.transition
    else {
        panic!("scalar append family")
    };
    assert_eq!(witness.value["finish_reason"]["type"], "other");
    assert!(
        witness.value["finish_reason"]["reason"]
            .as_str()
            .unwrap()
            .len()
            <= 256
    );
    assert!(witness.deferred);
    assert_eq!(receipt.read_cut.read.journal, 2);
    assert_eq!(
        source.read_cut.read.journal, 1,
        "later native writes cannot retag prior source evidence"
    );
    let summary = store.observation_attempt_summary(&attempt, 2).unwrap();
    assert!(summary.finish_reason_deferred);
    assert_eq!(summary.finish_reason_sequence, Some(2));
    assert_eq!(summary.terminal_sequence, Some(2));
    let fields = ["event".into(), "finish_reason".into(), "reason".into()];
    let mut offset = 0;
    let mut bytes = Vec::new();
    loop {
        let chunk = store
            .event_value_read(2, 2, &fields, offset, 65_536)
            .unwrap();
        let (rows, materialized) = SqliteConversationStore::content_read_work();
        assert!(rows <= 2 && materialized <= 131_072);
        bytes.extend(chunk.bytes);
        match chunk.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(serde_json::from_slice::<String>(&bytes).unwrap(), reason);
    let durable = serde_json::to_value(&settled).unwrap();
    assert!(durable.get("observation").is_none());
    assert_eq!(
        durable["event"]["finish_reason"]["reason"]
            .as_str()
            .unwrap(),
        reason
    );
}

/// T14/T19/T20: hidden Context uses its actual immutable C source, and a
/// giant `FreshInbound` identity keeps its native placement without body cloning.
#[test]
fn t14_t19_t20_status_receipt_preserves_giant_source_identity_and_placement() {
    let store = store();
    // Native model-head identity admission is <=4096 UTF-8 bytes.
    // Escaped original JSON still exceeds16 KiB and must use Source ranges.
    let native_id = format!("native-input{}", "\u{1}".repeat(3900));
    let base = user_message(&native_id, "ordinary user");
    store.initialize(std::slice::from_ref(&base)).unwrap();
    let time = Utc.with_ymd_and_hms(2026, 8, 27, 6, 0, 0).unwrap();
    let status = crate::context::AgentStatus {
        generated_at: time,
        sections: vec![crate::context::AgentStatusSection {
            id: crate::context::AgentStatusSectionId::new("temporal"),
            data: crate::context::AgentStatusSectionData::Temporal {
                current_time: time,
                timezone: None,
            },
        }],
    };
    let status_id = MessageId::new(format!("native-status{}", "\u{1}".repeat(3900)));
    let rendered = crate::context::render_agent_status(&status);
    let mut canonical =
        agent_status_message(status_id.as_str(), time, &[AgentStatusModuleId::Time]);
    let MessageBlock::User(user) = &mut canonical else {
        panic!("status User context")
    };
    user.content = vec![crate::message::types::UserContentBlock::Text(
        crate::message::content::TextBlock {
            text: rendered.clone(),
        },
    )];
    let mut snapshot = status_start_snapshot_for(
        "status-source-attempt",
        "1",
        store.load_head().unwrap().revision.next(),
        status_id.clone(),
        vec![],
        vec![status_id],
    );
    let contribution = snapshot.contributions.last_mut().unwrap();
    contribution.metadata = ContextKind::AgentStatus(status.generation_metadata());
    contribution.presentation =
        Some(crate::context::contribution::ContributionPresentation::AgentStatus(status));
    contribution.opportunities.fresh_inbound = Some(crate::context::FreshInboundOpportunity {
        target_message_id: base.id().clone(),
    });
    freeze_test_input(&store, &mut snapshot, std::slice::from_ref(&canonical));
    let committed = store
        .commit_model_turn_start(&[canonical], &snapshot, time)
        .unwrap();
    assert_eq!(committed.status_sources.len(), 1);
    let native = committed.started.observation.as_ref().unwrap();
    let crate::durable::observation::NativeObservationTransition::RequestStart { statuses, .. } =
        &native.transition
    else {
        panic!("closed RequestStart family")
    };
    assert_eq!(statuses.len(), 1);
    let view = &statuses[0];
    assert!(view.status_message_id.is_none());
    assert_eq!(
        view.source_locator,
        Some(
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position: 2 }
        )
    );
    let fresh = view.opportunities.fresh_inbound.as_ref().unwrap();
    assert!(fresh.target_message_id.is_none());
    assert_eq!(
        fresh.target_locator,
        Some(
            crate::observation::content::CanonicalContentLocator::ByLedgerPosition { position: 1 }
        )
    );
    assert_eq!(
        fresh.transcript_anchor,
        Some(crate::observation::transcript::RuntimeClientTranscriptCursor::new(1))
    );
    assert_eq!(native.read_cut.ledger_through, 2);
    assert_eq!(native.read_cut.read.journal, committed.started.sequence);
    assert!(serde_json::to_vec(view).unwrap().len() < 16_384);
    assert_status_original_ranges(&store, view, &rendered, base.id());
    assert_eq!(
        store.observation_read_cut().unwrap().transcript_total,
        1,
        "hidden status remains outside transcript"
    );
}
