use super::{RuntimeClientError, RuntimeClientProjection, SubscriberPoll};
use crate::agent::{
    AgentExecution, AgentExecutionObserver, AgentExecutionRequest, AgentStatusObservation,
};
use crate::context::{
    AgentStatus, AgentStatusEngine, CompactionBudgets, ContextEngine, ContextError,
    ContextErrorKind, ContextRuntime, ContributionOpportunities, FreshInboundOpportunity,
    PostToolBatchOpportunity,
};
use crate::conversation::ConversationState;
use crate::durable::AuxiliaryStore;
use crate::durable::CanonicalStore;
use crate::durable::JournalStore;
use crate::durable::ObservationStore;
use crate::durable::PublicationStore;
use crate::durable::RequestStore;
use crate::durable::inbox::InboxStore;
use crate::events::interaction::{InteractionSettlement, InteractionSubject};
use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
    ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::adapter::ModelAdapter;
use crate::model::error::{ModelError, ModelErrorKind};
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;
use crate::model::types::ModelUsage;
use crate::observation::status::{
    AgentStatusOpportunityView, AgentStatusView, RuntimeClientStatusSection,
};
use crate::observation::tool::ForegroundToolState;
use crate::observation::transcript::RuntimeClientTranscriptCursor;
use crate::publication::{PublicationFrame, PublicationPayload};
use crate::runtime::identity::{
    AgentId, AttemptId, ConversationId, EventId, InteractionId, MessageId, RequestId, ToolCallId,
    ToolId, TurnId,
};
use crate::runtime::inbound::{ConversationInboundMailbox, InitialTurnTrigger};
use crate::runtime::interaction::{
    ApprovalDecision, InteractionKind, InteractionOutcome, InteractionRef, InteractionRequest,
    InteractionResponse, InteractionSource, RoutedInteraction,
};
use crate::runtime::observation::ConversationObservation;
use crate::runtime::types::{CancellationReason, TokenMeasurement, TokenMeasurementSource};
use crate::runtime_client::event::{RuntimeClientEvent, RuntimeClientOutcome};
use crate::runtime_client::snapshot::{
    AGENT_STATUS_WINDOW, InFlightBlock, RuntimeClientAttemptPhase,
};
use crate::runtime_client::types::RuntimeClientCursor;
use crate::scripted_suites::support::context::{
    FakeContextSummarizer, FakeSummaryStep, ScriptedEstimator,
};
use crate::scripted_suites::support::fake::{FakeModel, FakeStep};
use crate::tools::executor::ToolRegistry;
use crate::tools::types::{
    ToolCall, ToolCallStart, ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus,
    ToolProgress,
};
use std::sync::{Arc, Mutex};

fn attempt() -> AttemptId {
    AttemptId::new("attempt-1")
}

fn interaction_audit(
    request: &InteractionRequest,
    event_id: &str,
    event: RuntimeEvent,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        tool_occurrence: None,
        observation: None,
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(event_id),
        sequence: 1,
        conversation_id: request.conversation_id.clone(),
        attempt_id: request.execution.attempt_id().cloned(),
        turn_id: request
            .execution
            .turn()
            .map(|turn| TurnId::new(turn.to_string())),
        timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc),
        event,
    }
}

/// A deterministic session model view for projection unit tests.
fn model_view() -> crate::model::session::SessionModelView {
    crate::scripted_suites::support::model::scripted_session_model(std::sync::Arc::new(
        crate::scripted_suites::support::model::NullAdapter,
    ))
    .view()
}

fn projection() -> RuntimeClientProjection {
    RuntimeClientProjection::new(
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        Vec::new(),
        crate::observation::capability_source::CapabilityView {
            configured_tools: Vec::new(),
            revision: crate::runtime::identity::CapabilityRevision::new(1),
            tools: Vec::new(),
            available_tools: Vec::new(),
            skills: Vec::new(),
            sources: Vec::new(),
        },
        Some(model_view()),
        64,
    )
}

/// Adoption eligibility is runtime-domain projection state: a published
/// change folds into the snapshot and publishes one event, and an
/// unchanged value folds into nothing. It invalidates no derived read
/// domain, because it is not conversation history.
#[test]
fn adoption_eligibility_publishes_only_changes() {
    use crate::runtime::adoption::AdoptionEligibility;
    let mut projection = projection();
    let (snapshot, cursor) = projection.snapshot().unwrap();
    assert_eq!(
        snapshot.configuration_adoption_eligibility,
        AdoptionEligibility::Unavailable
    );
    let (subscriber, _) = projection.subscribe(cursor).unwrap();
    for eligibility in [
        AdoptionEligibility::Unavailable,
        AdoptionEligibility::Eligible,
        AdoptionEligibility::Eligible,
        AdoptionEligibility::Busy,
        AdoptionEligibility::Busy,
        AdoptionEligibility::Eligible,
    ] {
        projection.apply(ConversationObservation::AdoptionEligibility(eligibility));
    }
    assert!(projection.read_domain_cut().is_none());
    let mut published = Vec::new();
    while let SubscriberPoll::Event(event) = projection.poll_subscriber(subscriber) {
        let crate::runtime_client::RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged {
            eligibility,
        } = single_fact(&event)
        else {
            panic!("only eligibility changes are published: {:?}", event.event);
        };
        published.push(eligibility.clone());
    }
    assert_eq!(
        published,
        vec![
            AdoptionEligibility::Eligible,
            AdoptionEligibility::Busy,
            AdoptionEligibility::Eligible
        ]
    );
    assert_eq!(
        projection
            .snapshot()
            .unwrap()
            .0
            .configuration_adoption_eligibility,
        AdoptionEligibility::Eligible
    );
}

#[test]
fn t14_t20_workflow_native_revision_gaps_preserve_complete_replay_and_checkpoint() {
    let event: RuntimeClientEvent = serde_json::from_str(include_str!(
        "../../../tests/fixtures/runtime-client/workflow-v22.json"
    ))
    .unwrap();
    let RuntimeClientEvent::WorkflowsUpdated { mut workflows } = event else {
        panic!("original native Workflow fixture");
    };
    let (mut projection, _store) = initialized_source_projection();
    let mut folded = projection.checkpoint_candidate().unwrap();
    let initial = folded.clone();
    let (subscriber, _) = projection.subscribe(initial.cursor).unwrap();
    for revision in [37, 41] {
        workflows.revision.0 = revision;
        let source = crate::observation::workflow_source::WorkflowReadSource::new(Arc::new(
            workflows.clone(),
        ));
        source.prepare();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::Workflow(vec![source]));
        });
        let SubscriberPoll::Event(record) = projection.poll_subscriber(subscriber) else {
            panic!("a complete native replacement remains replayable");
        };
        assert_eq!(record.cursor.get(), folded.cursor.get() + 1);
        assert!(
            projection
                .replay
                .iter()
                .any(|(cursor, event)| { *cursor == record.cursor && event == &record.event })
        );
        let RuntimeClientEvent::ProjectionTransition { events, metadata } = &record.event else {
            panic!("one complete Runtime transition");
        };
        let [
            RuntimeClientEvent::WorkflowsUpdated {
                workflows: replacement,
            },
        ] = events.as_slice()
        else {
            panic!("one positive Workflow replacement");
        };
        assert_eq!(replacement.revision.0, revision);
        folded.snapshot.workflows.clone_from(replacement);
        folded.cursor = record.cursor;
        folded.read_cut.clone_from(&metadata.read_cut);
        folded.collections.clone_from(&metadata.collections);
        folded.content.clone_from(&metadata.content);
        assert_eq!(folded, projection.checkpoint_candidate().unwrap());
        projection
            .validate_checkpoint_cut(&initial.read_cut)
            .unwrap();
        assert!(projection.initial_read_cut.is_some());
        assert_eq!(
            projection.replay.len(),
            usize::try_from(record.cursor.get()).unwrap()
        );
    }
    assert_eq!(projection.replay.len(), 2);
    assert_eq!(projection.snapshot_ref().workflows.revision.0, 41);
    assert!(
        projection
            .validate_domain_version(&initial.read_cut, "workflows.runs")
            .is_err()
    );
}

#[test]
fn t14_t15_workflow_subdomain_page_and_value_use_the_same_native_owner_cut() {
    use base64::Engine as _;
    let event: RuntimeClientEvent = serde_json::from_str(include_str!(
        "../../../tests/fixtures/runtime-client/workflow-v22.json"
    ))
    .unwrap();
    let RuntimeClientEvent::WorkflowsUpdated {
        workflows: original,
    } = event
    else {
        panic!("original native Workflow fixture");
    };
    assert!(!original.runs.is_empty());
    let source =
        crate::observation::workflow_source::WorkflowReadSource::new(Arc::new(original.clone()));
    source.prepare();
    let (mut projection, _store) = initialized_source_projection();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::Workflow(vec![source]));
    });
    let cut = projection.checkpoint_candidate().unwrap().read_cut;
    projection.validate_checkpoint_cut(&cut).unwrap();
    projection
        .validate_domain_version(&cut, "workflows.runs")
        .unwrap();
    let selected = projection
        .domain_source("workflows.runs", 0)
        .expect("subdomain selects the existing native Workflow owner");
    assert!(Arc::ptr_eq(
        &selected,
        &projection.domain_source("workflows", 0).unwrap()
    ));
    let page = selected
        .page(&cut, "workflows.runs", &["runs".into()], 0, 32, false)
        .unwrap();
    assert_eq!(page.read_cut, cut);
    assert_eq!(page.cursor.get(), cut.revision);
    assert_eq!(page.total, u64::try_from(original.runs.len()).unwrap());
    assert_eq!(page.items[0].offset, 0);
    let chunk = selected
        .read(
            &cut,
            "workflows.runs",
            0,
            &["runs".into(), "@0".into()],
            0,
            65_536,
        )
        .unwrap();
    assert_eq!(chunk.cut, cut);
    assert_eq!(chunk.offset, 0);
    assert_eq!(chunk.next_offset, None);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(chunk.bytes_base64)
        .unwrap();
    assert_eq!(u64::try_from(bytes.len()).unwrap(), chunk.readable_end);
    let run: crate::runtime::workflow::read_model::WorkflowRunView =
        crate::observation::bounded::decode_domain_value(serde_json::from_slice(&bytes).unwrap())
            .unwrap();
    assert_eq!(run, original.runs[0]);

    let mut later = original;
    later.revision.0 += 1;
    let source = crate::observation::workflow_source::WorkflowReadSource::new(Arc::new(later));
    source.prepare();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::Workflow(vec![source]));
    });
    assert!(matches!(
        projection.validate_domain_version(&cut, "workflows.runs"),
        Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
    ));
}

#[test]
fn queued_old_review_cannot_resurrect_a_settled_workflow_node() {
    let mut client = projection();
    let request: crate::runtime::interaction::InteractionRequest = serde_json::from_str(
        include_str!("../../../tests/fixtures/runtime-client/review-v21.json"),
    )
    .unwrap();
    let crate::runtime::interaction::InteractionKind::Review { review, .. } = &request.kind else {
        panic!("Review fixture");
    };
    let node = (*review.instance).clone();
    let event: crate::runtime_client::RuntimeClientEvent = serde_json::from_str(include_str!(
        "../../../tests/fixtures/runtime-client/workflow-v22.json"
    ))
    .unwrap();
    let crate::runtime_client::RuntimeClientEvent::WorkflowsUpdated { mut workflows } = event
    else {
        panic!("Workflow fixture");
    };
    workflows.runs[0].id.clone_from(&node.block.run);
    workflows.runs[0].instances[0].block = node.block;
    workflows.runs[0].instances[0].node = Some(node.node);
    client
        .snapshot
        .pending_interactions
        .push(crate::observation::interaction::interaction_header(
            &crate::runtime::interaction::RoutedInteraction::primary(request),
        ));
    client.fold_workflows(workflows);
    let row = &client.snapshot.workflows.runs[0].instances[0];
    assert!(matches!(
        row.state,
        crate::runtime::workflow::read_model::WorkflowState::Settled { .. }
    ));
    assert!(row.interaction.is_none());
}

fn apply_event(projection: &mut RuntimeClientProjection, event: RuntimeEvent) {
    projection.apply(event_observation(event));
}

#[test]
fn loop_facts_remain_journal_observations_without_client_execution_or_history() {
    use crate::runtime::workflow::{WorkflowExecutionOutcome, WorkflowLoopExit, test_instance};
    let mut projection = projection();
    let (before, before_cursor) = projection.snapshot().unwrap();
    let node = test_instance("feedback", "loop");
    let body = node.block.clone();
    for event in [
        RuntimeEvent::WorkflowLoopIterationAdmitted {
            node: node.clone(),
            body: body.clone(),
            iteration: 1,
        },
        RuntimeEvent::WorkflowLoopIterationSettled {
            node: node.clone(),
            body,
            iteration: 1,
            outcome: WorkflowExecutionOutcome::Completed,
        },
        RuntimeEvent::WorkflowLoopExited {
            node,
            iterations: 1,
            status: WorkflowLoopExit::Exhausted,
        },
    ] {
        projection.apply(event_observation(event));
    }
    let (after, after_cursor) = projection.snapshot().unwrap();
    assert_eq!(before_cursor, after_cursor);
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
}

/// Opens the representative publication stream on a projection.
fn apply_publication_open(projection: &mut RuntimeClientProjection) {
    projection.apply(ConversationObservation::PublicationOpened {
        attempt_id: attempt(),
        start: stream_start(),
    });
}

/// Releases one already-committed publication frame into a projection.
fn apply_frame(
    projection: &mut RuntimeClientProjection,
    sequence: u64,
    payload: PublicationPayload,
) {
    if projection.publication_start.is_none() {
        apply_publication_open(projection);
    }
    projection.apply(ConversationObservation::Publication {
        attempt_id: attempt(),
        frame: frame(sequence, payload),
    });
}

fn text_frame(text: &str) -> PublicationPayload {
    PublicationPayload::TextSuffix {
        block_index: ContentBlockIndex::new(0),
        suffix: text.to_owned(),
    }
}

fn event_observation(event: RuntimeEvent) -> ConversationObservation {
    ConversationObservation::Event {
        attempt_id: attempt(),
        event,
    }
}

fn auxiliary_witness() -> crate::durable::observation::ObservationEventWitness {
    let call_id =
        crate::runtime::identity::AuxiliaryCallId::new("aux_36524fd8-f674-7fc2-8125-06d01fee0e18");
    crate::durable::observation::ObservationEventWitness {
        occupancy: None,
        native_direct: false,
        trace_total: Some(0),
        source_unavailable: false,
        sequence: 1,
        transcript_cursor: None,
        event_id: None,
        attempt_id: None,
        turn_id: None,
        owner: Some(crate::durable::observation::RequestOwnerKind::Auxiliary),
        auxiliary_call_id: Some(call_id),
        timestamp: chrono::DateTime::UNIX_EPOCH,
        event_kind: "model_request_started".into(),
        tool_occurrence: None,
        result_ledger_position: None,
        value: serde_json::json!({"type": "model_request_started"}),
        fields: Vec::new(),
        deferred: false,
    }
}

#[test]
fn auxiliary_journal_fact_advances_only_its_audit_domain() {
    let mut projection = projection();
    projection.set_auxiliary_audit_total(7);
    let before_snapshot = projection.snapshot().unwrap().0;
    let before = |projection: &RuntimeClientProjection, domain| projection.domain_revision(domain);
    let attempt = before(&projection, "attempt");
    let context = before(&projection, "context");
    let trace = before(&projection, "trace");
    let auxiliary = before(&projection, "auxiliary");
    projection.advance_domain_revisions(&ConversationObservation::JournalFact(auxiliary_witness()));
    assert_eq!(projection.domain_revision("auxiliary"), auxiliary + 1);
    assert_eq!(projection.domain_revision("attempt"), attempt);
    assert_eq!(projection.domain_revision("context"), context);
    assert_eq!(projection.domain_revision("trace"), trace);
    assert!(!super::observe::invalidates_read_domains(
        &ConversationObservation::JournalFact(auxiliary_witness())
    ));
    let events = projection.fold_journal_fact(auxiliary_witness());
    assert!(matches!(
        events.as_slice(),
        [RuntimeClientEvent::CollectionChanged { domain, cut }]
            if domain == crate::runtime_client::auxiliary::DOMAIN
                && cut.total == 8
    ));
    assert_eq!(projection.snapshot().unwrap().0, before_snapshot);
    let checkpoint = projection.checkpoint_candidate().unwrap();
    assert!(
        checkpoint
            .collections
            .iter()
            .all(|frontier| frontier.domain != crate::runtime_client::auxiliary::DOMAIN)
    );
}

#[test]
fn auxiliary_native_direct_fact_still_advances_only_the_audit_domain() {
    let mut projection = projection();
    projection.set_auxiliary_audit_total(3);
    let attempt = projection.domain_revision("attempt");
    let read_revision = projection.read_revision;
    let mut witness = auxiliary_witness();
    witness.native_direct = true;
    projection.apply(ConversationObservation::JournalFact(witness));
    assert_eq!(projection.domain_revision("auxiliary"), 1);
    assert_eq!(projection.domain_revision("attempt"), attempt);
    assert_eq!(projection.auxiliary_audit_total, Some(4));
    assert_eq!(projection.read_revision, read_revision);
    assert!(!projection.read_domains_dirty);
}

#[test]
#[allow(clippy::too_many_lines)] // Exercise actual Store receipts and the complete live projection path.
fn r1_primary_request_usage_survives_reduced_native_witness_and_auxiliary_interleave() {
    use crate::durable::observation::NativeObservationTransition;
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart,
        AuxiliaryFingerprint, AuxiliaryInputEvidence, AuxiliaryObservedProviderTerminal,
        AuxiliaryRequestEvidence, AuxiliaryRequestStart, AuxiliaryUsageCompleteness,
    };
    use crate::model::deadline::ModelTimeoutPhase;
    use crate::model::snapshot::ModelRequestOwner;
    use crate::runtime::identity::{AuxiliaryCallId, SystemUuidV7Generator};

    for (completed, primary_usage) in [
        (
            true,
            ModelUsage {
                input_tokens: Some(101),
                output_tokens: Some(23),
                total_tokens: Some(124),
                details: None,
            },
        ),
        (
            false,
            ModelUsage {
                input_tokens: Some(211),
                output_tokens: Some(37),
                total_tokens: Some(248),
                details: None,
            },
        ),
        (
            true,
            ModelUsage {
                input_tokens: Some(u64::MAX - 23),
                output_tokens: Some(23),
                total_tokens: Some(u64::MAX),
                details: None,
            },
        ),
    ] {
        let expected_usage = completed.then_some(primary_usage.clone());
        let mut projection = projection();
        let store = SqliteConversationStore::in_memory(projection.snapshot.conversation_id.clone())
            .unwrap();
        projection
            .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
            .unwrap();
        projection.initialize_publication_view();

        let attempt_started = store
            .append_event(source_tool_event(
                &store,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt(),
                },
                "r1-primary-attempt-start",
            ))
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(attempt_started.observation.as_deref().unwrap());
        });

        let snapshot = raw_publication_snapshot(crate::context::resources::LedgerFrontier(
            store.observation_read_cut().unwrap().ledger_through,
        ));
        let started = store
            .commit_model_turn_start(&[], &snapshot, chrono::Utc::now())
            .unwrap();
        let started_native = started.started.observation.as_deref().unwrap();
        let NativeObservationTransition::RequestStart {
            started: started_witness,
            ..
        } = &started_native.transition
        else {
            panic!("primary request start has its native request transition");
        };
        assert_eq!(
            started_witness.owner,
            Some(crate::durable::observation::RequestOwnerKind::Agent)
        );
        assert!(started_witness.value["owner"].get("identity").is_none());
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(started_native);
        });

        let owner = ModelRequestOwner::Agent {
            identity: snapshot.identity.clone(),
        };
        let terminal = if completed {
            RuntimeEvent::ModelRequestCompleted {
                owner,
                request_id: snapshot.request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: Some(primary_usage.clone()),
                generation: None,
            }
        } else {
            RuntimeEvent::ModelRequestFailed {
                owner,
                request_id: snapshot.request_id.clone(),
                error: ModelError::timeout(ModelTimeoutPhase::ResponseStart),
                usage: Some(primary_usage.clone()),
                generation: None,
            }
        };
        let terminal = store
            .append_event(source_tool_event(
                &store,
                terminal,
                if completed {
                    "r1-primary-request-completed"
                } else {
                    "r1-primary-request-failed"
                },
            ))
            .unwrap();
        let terminal_native = terminal.observation.as_deref().unwrap();
        let NativeObservationTransition::Single(terminal_witness) = &terminal_native.transition
        else {
            panic!("primary request terminal has one native witness");
        };
        assert_eq!(
            terminal_witness.owner,
            Some(crate::durable::observation::RequestOwnerKind::Agent)
        );
        assert!(terminal_witness.value["owner"].get("identity").is_none());
        assert!(
            crate::observation::structural_json::decode::<RuntimeEvent>(
                terminal_witness.value.clone()
            )
            .is_err(),
            "Reduced display witnesses must not satisfy authoritative event identity requirements"
        );
        assert_eq!(
            crate::observation::bounded::decode_domain_value::<ModelUsage>(
                terminal_witness.value["usage"].clone(),
            )
            .unwrap(),
            primary_usage
        );

        let before_terminal = projection.cursor;
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(terminal_native);
        });
        let terminal_events = collect(&mut projection, before_terminal);
        let usage_events = terminal_events
            .iter()
            .flat_map(|event| event.event.facts())
            .filter_map(|fact| match fact {
                RuntimeClientEvent::AttemptUsageUpdated { attempt_id, usage } => {
                    Some((attempt_id.clone(), usage.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            usage_events,
            if completed {
                vec![(attempt(), primary_usage.clone())]
            } else {
                Vec::new()
            },
            "Only completed primary requests update live attempt Usage"
        );
        assert_eq!(
            projection.snapshot().unwrap().0.attempt.unwrap().last_usage,
            expected_usage
        );

        let auxiliary_usage = ModelUsage {
            input_tokens: Some(7),
            output_tokens: Some(3),
            total_tokens: Some(10),
            details: None,
        };
        let call_id = AuxiliaryCallId::generate();
        let auxiliary_request_id = RequestId::generate_auxiliary(&SystemUuidV7Generator).unwrap();
        let call_started = store
            .commit_auxiliary_call_start(
                &AuxiliaryCallStart {
                    call_id: call_id.clone(),
                    purpose: AuxiliaryAttribution::new("summary").unwrap(),
                    trigger: AuxiliaryAttribution::new("automatic").unwrap(),
                    business_source: AuxiliaryBusinessSource::Independent,
                    input: AuxiliaryInputEvidence::Temporary,
                    semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
                    invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
                    selected_message_ids: Vec::new(),
                },
                chrono::Utc::now(),
            )
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(call_started.event.observation.as_deref().unwrap());
        });
        let request_started = store
            .commit_auxiliary_request_start(
                &AuxiliaryRequestStart {
                    call_id: call_id.clone(),
                    ordinal: 0,
                    request_id: auxiliary_request_id.clone(),
                    model: "fixture/auxiliary".into(),
                },
                chrono::Utc::now(),
            )
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(request_started.event.observation.as_deref().unwrap());
        });
        let auxiliary_terminal = crate::durable::auxiliary::auxiliary_settlement_envelope(
            store.conversation_id().clone(),
            chrono::Utc::now(),
            RuntimeEvent::ModelRequestCompleted {
                owner: ModelRequestOwner::Auxiliary {
                    call_id: call_id.clone(),
                    ordinal: 0,
                },
                request_id: auxiliary_request_id.clone(),
                finish_reason: ModelFinishReason::Stop,
                usage: Some(auxiliary_usage.clone()),
                generation: None,
            },
        )
        .unwrap();
        let auxiliary_settled = crate::durable::auxiliary::auxiliary_settlement_envelope(
            store.conversation_id().clone(),
            chrono::Utc::now(),
            RuntimeEvent::AuxiliaryCallSettled {
                call_id,
                outcome: AuxiliaryCallOutcome::Completed,
                request_evidence: Some(AuxiliaryRequestEvidence {
                    request_id: auxiliary_request_id,
                    observed_provider_terminal: Some(
                        AuxiliaryObservedProviderTerminal::Completed {
                            finish_reason: crate::model::auxiliary::AuxiliaryFinishReason::Stop,
                        },
                    ),
                    usage: Some(auxiliary_usage),
                    usage_completeness: AuxiliaryUsageCompleteness::Complete,
                    remote_execution_uncertainty: false,
                }),
            },
        )
        .unwrap();
        let auxiliary = store
            .commit_auxiliary_request_settlement(auxiliary_terminal, auxiliary_settled)
            .unwrap();
        let auxiliary_native = auxiliary
            .request_terminal
            .as_ref()
            .unwrap()
            .observation
            .as_deref()
            .unwrap();
        let NativeObservationTransition::Single(auxiliary_witness) = &auxiliary_native.transition
        else {
            panic!("auxiliary request terminal has one native witness");
        };
        assert_eq!(
            auxiliary_witness.owner,
            Some(crate::durable::observation::RequestOwnerKind::Auxiliary)
        );
        let before_auxiliary = projection.cursor;
        publish_source_fact(&mut projection, |projection| {
            projection.apply_native_batch(auxiliary_native);
            projection.apply_native_batch(auxiliary.call_settled.observation.as_deref().unwrap());
        });
        let auxiliary_events = collect(&mut projection, before_auxiliary);
        assert!(auxiliary_events.iter().flat_map(|event| event.event.facts()).any(|fact|
            matches!(fact, RuntimeClientEvent::CollectionChanged { domain, .. } if domain == "auxiliary.audit")
        ), "The auxiliary receipt must actually reach its audit projection");
        assert!(
            auxiliary_events
                .iter()
                .flat_map(|event| event.event.facts())
                .all(|fact| !matches!(fact, RuntimeClientEvent::AttemptUsageUpdated { .. }))
        );
        assert_eq!(
            projection.snapshot().unwrap().0.attempt.unwrap().last_usage,
            expected_usage,
            "Auxiliary Usage must not replace primary attempt Usage"
        );
    }
}

/// One composed status observation for the placement suites.
///
/// `post_tool_batch_anchor` is the placement fact the Agent Loop froze at
/// the tool batch commit; it is supplied by the caller exactly as the
/// loop supplies it, never derived from projection state.
fn status_observation(
    status_message_id: &str,
    turn: u32,
    opportunities: ContributionOpportunities,
    post_tool_batch_anchor: Option<crate::durable::TranscriptCursor>,
) -> AgentStatusObservation {
    AgentStatusObservation {
        attempt_id: attempt(),
        turn,
        status_message_id: MessageId::new(status_message_id),
        opportunities,
        post_tool_batch_anchor,
        status: AgentStatus {
            generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
            sections: Vec::new(),
        },
    }
}

/// One `FreshInbound`-only composition: placed by the exact inbound
/// message identity, which needs no transcript position.
fn fresh_inbound_status(
    status_message_id: &str,
    turn: u32,
    target: &str,
) -> AgentStatusObservation {
    status_observation(
        status_message_id,
        turn,
        ContributionOpportunities {
            fresh_inbound: Some(FreshInboundOpportunity {
                target_message_id: MessageId::new(target),
            }),
            post_tool_batch: None,
        },
        None,
    )
}

/// One `PostToolBatch`-only composition, carrying the durable position
/// its settled tool batch froze.
fn post_tool_batch_status(
    status_message_id: &str,
    turn: u32,
    batch_cursor: u64,
) -> AgentStatusObservation {
    status_observation(
        status_message_id,
        turn,
        ContributionOpportunities {
            fresh_inbound: None,
            post_tool_batch: Some(PostToolBatchOpportunity),
        },
        Some(crate::durable::TranscriptCursor::new(batch_cursor)),
    )
}

/// The transcript-position placement one composition publishes, if any.
fn published_anchor(view: &AgentStatusView) -> Option<RuntimeClientTranscriptCursor> {
    view.opportunities
        .post_tool_batch
        .and_then(|batch| batch.transcript_anchor)
}

/// Commits one visible canonical message at a durable transcript
/// position.
fn commit_at(projection: &mut RuntimeClientProjection, id: &str, cursor: u64) {
    projection.apply(ConversationObservation::Committed {
        attempt_id: Some(attempt()),
        block: MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "hello".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        }),
        transcript_cursor: Some(crate::durable::TranscriptCursor::new(cursor)),
    });
}

/// A `FreshInbound` composition is placed by the exact inbound message
/// identity its opportunity carries, and publishes no transcript
/// position: a message identity is the stronger fact, and nothing later
/// can move it.
#[test]
fn fresh_inbound_status_is_placed_by_message_identity() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    commit_at(&mut projection, "user-1", 7);
    projection.apply(ConversationObservation::Status(
        crate::observation::status::status_view(&(fresh_inbound_status("status-1", 1, "user-1"))),
    ));

    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.statuses.len(), 1);
    assert_eq!(
        snapshot.statuses[0]
            .opportunities
            .fresh_inbound
            .as_ref()
            .expect("FreshInbound view")
            .target_message_id,
        Some(MessageId::new("user-1"))
    );
    assert_eq!(
        published_anchor(&snapshot.statuses[0]),
        None,
        "a FreshInbound composition needs no transcript position"
    );

    // A later commit changes nothing about an already-composed status.
    commit_at(&mut projection, "user-2", 9);
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.statuses[0]
            .opportunities
            .fresh_inbound
            .as_ref()
            .expect("FreshInbound view")
            .target_message_id,
        Some(MessageId::new("user-1"))
    );
}

/// A `PostToolBatch` composition publishes exactly the placement the
/// Agent Loop froze at its tool batch commit — **not** whatever durable
/// position the conversation happens to have reached when the projection
/// folds the observation.
///
/// This is the invariant that a fold-time frontier violated. Inbound
/// acceptance is an independent durable boundary and may legally commit
/// between the composition and its observation; the interleaving below is
/// exactly that case, folded in the order it can really occur.
#[test]
fn post_tool_batch_status_keeps_the_anchor_its_batch_froze() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    commit_at(&mut projection, "user-1", 3);
    commit_at(&mut projection, "tool-result", 5);

    // Unrelated durable transcript activity lands before the status
    // observation reaches the projection.
    commit_at(&mut projection, "unrelated-inbound", 6);

    projection.apply(ConversationObservation::Status(
        crate::observation::status::status_view(
            &(post_tool_batch_status("status-post-batch", 2, 5)),
        ),
    ));

    let (snapshot, _) = projection.snapshot().expect("snapshot");
    let status = snapshot.statuses.last().expect("composed status");
    assert!(status.opportunities.fresh_inbound.is_none());
    assert_eq!(
        published_anchor(status),
        Some(RuntimeClientTranscriptCursor::new(5)),
        "the anchor is the tool batch's own frozen position, not the newest durable one"
    );
}

/// A new attempt is not a retraction: an earlier composition stays in the
/// window, at its own placement, and a later one is added beside it.
#[test]
fn a_later_attempt_neither_removes_nor_relocates_an_earlier_status() {
    let mut projection = projection();
    commit_at(&mut projection, "user-1", 1);
    projection.apply(ConversationObservation::Status(
        crate::observation::status::status_view(&(fresh_inbound_status("status-1", 1, "user-1"))),
    ));
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    commit_at(&mut projection, "tool-result", 4);
    projection.apply(ConversationObservation::Status(
        crate::observation::status::status_view(&(post_tool_batch_status("status-2", 1, 4))),
    ));

    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .statuses
            .iter()
            .map(|status| status
                .status_message_id
                .as_ref()
                .expect("inline status identity")
                .as_str()
                .to_owned())
            .collect::<Vec<_>>(),
        vec!["status-1".to_owned(), "status-2".to_owned()]
    );
    assert_eq!(
        snapshot.statuses[0]
            .opportunities
            .fresh_inbound
            .as_ref()
            .expect("FreshInbound view")
            .target_message_id,
        Some(MessageId::new("user-1"))
    );
    assert_eq!(
        published_anchor(&snapshot.statuses[1]),
        Some(RuntimeClientTranscriptCursor::new(4))
    );
}

/// Identity is the composed status message: observing one composition
/// twice describes one historical fact, evicts nothing, and the window
/// stays bounded.
#[test]
fn the_agent_status_window_is_identity_keyed_and_bounded() {
    let mut projection = projection();
    let mut client = LiveStatusClient::attach(&mut projection);
    let observation = post_tool_batch_status("status-1", 1, 1);
    projection.apply(ConversationObservation::Status(
        crate::observation::status::status_view(&(observation.clone())),
    ));
    projection.apply(ConversationObservation::Status(
        crate::observation::status::status_view(&(observation)),
    ));
    client.drain(&mut projection);
    assert_eq!(
        client.evictions,
        vec![None, None],
        "neither the first admission nor its replay evicts anything"
    );
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.statuses.len(), 1);

    for index in 0..AGENT_STATUS_WINDOW + 5 {
        projection.apply(ConversationObservation::Status(
            crate::observation::status::status_view(
                &(post_tool_batch_status(
                    &format!("status-fill-{index}"),
                    1,
                    u64::try_from(index).expect("test index fits a cursor") + 2,
                )),
            ),
        ));
    }
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.statuses.len(), AGENT_STATUS_WINDOW);
    assert_eq!(
        snapshot.statuses.last().expect("newest").status_message_id,
        Some(MessageId::new(format!(
            "status-fill-{}",
            AGENT_STATUS_WINDOW + 4
        )))
    );
}

/// A continuously subscribed client that folds status-window
/// transitions exactly as the TUI reducer does.
///
/// It is deliberately dumb: it removes the identity the runtime says its
/// own admission evicted, then appends by identity. It does not know
/// `AGENT_STATUS_WINDOW`, does not count entries, and makes no retention
/// decision of its own.
struct LiveStatusClient {
    subscriber: u64,
    statuses: Vec<AgentStatusView>,
    evictions: Vec<Option<MessageId>>,
}

impl LiveStatusClient {
    fn attach(projection: &mut RuntimeClientProjection) -> Self {
        let (subscriber, _notify) = projection
            .subscribe(RuntimeClientCursor::new(0))
            .expect("serviceable cursor");
        Self {
            subscriber,
            statuses: Vec::new(),
            evictions: Vec::new(),
        }
    }

    /// Drains everything published so far and folds it.
    fn drain(&mut self, projection: &mut RuntimeClientProjection) {
        loop {
            match projection.poll_subscriber(self.subscriber) {
                SubscriberPoll::Event(event) => {
                    for fact in event.event.facts() {
                        let RuntimeClientEvent::AgentStatusComposed {
                            status,
                            evicted_status_message_id,
                            ..
                        } = fact
                        else {
                            continue;
                        };
                        self.evictions.push(evicted_status_message_id.clone());
                        if let Some(evicted) = evicted_status_message_id {
                            self.statuses.retain(|existing| {
                                existing.status_message_id.as_ref() != Some(evicted)
                            });
                        }
                        if !self
                            .statuses
                            .iter()
                            .any(|existing| existing.status_message_id == status.status_message_id)
                        {
                            self.statuses.push(status.clone());
                        }
                    }
                }
                SubscriberPoll::Pending => return,
                other => panic!("the status client lost its subscription: {other:?}"),
            }
        }
    }
}

/// **The bounded-window convergence invariant.**
///
/// Folding every live event through cursor `C` and replacing state from
/// the authoritative snapshot at cursor `C` must reconstruct the same
/// semantic presentation state — past the retention bound as well as
/// below it.
///
/// The client here is deliberately dumb: it applies the eviction each
/// event carries and appends by identity. It does not know
/// `AGENT_STATUS_WINDOW`, does not count, and makes no retention
/// decision, which is the entire point — with two retention owners this
/// fold and the snapshot would diverge from the 65th distinct
/// composition onwards.
#[test]
fn the_status_window_converges_between_live_fold_and_snapshot_repair() {
    let mut projection = projection();
    let mut live = LiveStatusClient::attach(&mut projection);
    let overshoot = 7;

    for index in 0..AGENT_STATUS_WINDOW + overshoot {
        let cursor = u64::try_from(index).expect("test index fits a cursor") + 1;
        projection.apply(ConversationObservation::Status(
            crate::observation::status::status_view(
                &(post_tool_batch_status(&format!("status-{index}"), 1, cursor)),
            ),
        ));
        live.drain(&mut projection);
    }
    let client = live.statuses;

    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.statuses.len(),
        AGENT_STATUS_WINDOW,
        "the runtime window stays bounded"
    );
    assert_eq!(
        client, snapshot.statuses,
        "the live fold reconstructs the authoritative window exactly"
    );
    assert_eq!(
        client
            .iter()
            .map(|status| status
                .status_message_id
                .as_ref()
                .expect("inline status identity")
                .as_str()
                .to_owned())
            .collect::<Vec<_>>(),
        (overshoot..AGENT_STATUS_WINDOW + overshoot)
            .map(|index| format!("status-{index}"))
            .collect::<Vec<_>>(),
        "retention and order are the runtime's, applied verbatim"
    );
    assert_eq!(
        client.last().map(|status| status.status_message_id.clone()),
        snapshot
            .statuses
            .last()
            .map(|status| status.status_message_id.clone()),
        "the newest composition agrees on both paths"
    );
    for (folded, authoritative) in client.iter().zip(&snapshot.statuses) {
        assert_eq!(
            published_anchor(folded),
            published_anchor(authoritative),
            "placement survives the fold unchanged"
        );
    }
}

/// Replay combined with eviction: re-observing a composition the window
/// already holds must not displace another one.
///
/// This is the failure a client-side retention rule invites — a second
/// "append then trim" would drop the oldest entry on every replay, while
/// the runtime's own window would not move at all.
#[test]
fn replaying_a_composition_evicts_nothing_from_a_full_window() {
    let mut projection = projection();
    let mut live = LiveStatusClient::attach(&mut projection);
    let mut observations = Vec::new();

    for index in 0..AGENT_STATUS_WINDOW + 3 {
        let cursor = u64::try_from(index).expect("test index fits a cursor") + 1;
        let observation = post_tool_batch_status(&format!("status-{index}"), 1, cursor);
        observations.push(observation.clone());
        projection.apply(ConversationObservation::Status(
            crate::observation::status::status_view(&(observation)),
        ));
        live.drain(&mut projection);
    }
    let (before, _) = projection.snapshot().expect("snapshot");
    let evictions_before = live.evictions.len();

    // Replay the newest composition and one that is still retained.
    for observation in [
        observations.last().expect("newest").clone(),
        observations[AGENT_STATUS_WINDOW].clone(),
    ] {
        projection.apply(ConversationObservation::Status(
            crate::observation::status::status_view(&(observation)),
        ));
        live.drain(&mut projection);
    }
    assert_eq!(
        live.evictions[evictions_before..],
        [None, None],
        "a replay is not an admission and evicts nothing"
    );

    let (after, _) = projection.snapshot().expect("snapshot");
    assert_eq!(after.statuses, before.statuses, "replay moves no window");
    assert_eq!(
        live.statuses, after.statuses,
        "the client fold agrees after replay"
    );
}

// -----------------------------------------------------------------------
// The PostToolBatch anchor race.
//
// The Agent Loop's status linearization is:
//
// ```text
// canonical ToolResult batch commits          <- placement determined here
//   -> pending PostToolBatch opportunity
//   -> next primary-step preparation
//   -> Agent Status composed
//   -> durable model-turn-start commit
//   -> observe_status                          <- placement observed here
// ```
//
// Inbound acceptance is an independent durable boundary and may commit
// anywhere in that window. The test below parks the loop at
// `observe_status`, durably accepts an unrelated inbound message while it
// is parked, folds that acceptance into the Runtime Client projection, and
// only then releases the status. Anything that derived placement at the
// fold would produce the inbound's cursor; the frozen fact does not.
// -----------------------------------------------------------------------

/// An observer that folds every execution fact into a Runtime Client
/// projection, but parks the first `PostToolBatch` status observation
/// before it reaches the projection.
///
/// The park is a real rendezvous: `observe_status` is a synchronous
/// Agent Loop callback, so blocking in it blocks the loop itself. The
/// controlling task therefore provably runs *between* the composition and
/// its publication — not merely at some convenient moment.
struct StatusParkObserver {
    projection: Arc<Mutex<RuntimeClientProjection>>,
    /// Every observed canonical commit with a durable position.
    commits: Mutex<Vec<(MessageId, crate::durable::TranscriptCursor)>>,
    /// Set when the loop is parked inside `observe_status`.
    parked: tokio::sync::watch::Sender<bool>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    released: Mutex<bool>,
}

impl StatusParkObserver {
    fn install(
        projection: Arc<Mutex<RuntimeClientProjection>>,
    ) -> (
        Arc<Self>,
        tokio::sync::watch::Receiver<bool>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (parked, parked_rx) = tokio::sync::watch::channel(false);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        (
            Arc::new(Self {
                projection,
                commits: Mutex::new(Vec::new()),
                parked,
                release: Mutex::new(release_rx),
                released: Mutex::new(false),
            }),
            parked_rx,
            release_tx,
        )
    }

    fn apply(&self, observation: ConversationObservation) {
        self.projection
            .lock()
            .expect("projection lock")
            .apply(observation);
    }

    /// The durable position of the last committed tool message: the
    /// canonical `ToolResult` batch's own transcript frontier.
    fn tool_batch_cursor(&self) -> crate::durable::TranscriptCursor {
        *self
            .commits
            .lock()
            .expect("commit lock")
            .iter()
            .filter(|(id, _)| id.as_str().contains("-tool-"))
            .map(|(_, cursor)| cursor)
            .max()
            .expect("the attempt committed a canonical ToolResult batch")
    }
}

impl AgentExecutionObserver for StatusParkObserver {
    fn observe_event(
        &self,
        attempt_id: &AttemptId,
        event: &RuntimeEvent,
        _journal_sequence: u64,
        _tool_occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
        _receipt: Option<&crate::durable::observation::NativeObservationBatch>,
    ) {
        self.apply(ConversationObservation::Event {
            attempt_id: attempt_id.clone(),
            event: event.clone(),
        });
    }

    fn observe_committed(
        &self,
        attempt_id: &AttemptId,
        block: &MessageBlock,
        receipt: &crate::durable::TranscriptCommitReceipt,
    ) {
        if let Some(cursor) = receipt.transcript_cursor {
            self.commits
                .lock()
                .expect("commit lock")
                .push((crate::conversation::message_id_of(block), cursor));
        }
        self.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: Some(attempt_id.clone()),
            receipt: receipt.clone(),
        });
    }

    fn observe_status(
        &self,
        observation: &AgentStatusObservation,
        _source: Option<&crate::durable::StatusObservationSource>,
    ) {
        let park = observation.opportunities.post_tool_batch.is_some()
            && observation.opportunities.fresh_inbound.is_none()
            && !*self.released.lock().expect("release flag lock");
        if park {
            self.parked.send_replace(true);
            // Blocks the Agent Loop here, before the Runtime Client can
            // fold anything about this composition.
            let _ = self.release.lock().expect("release lock").recv();
            *self.released.lock().expect("release flag lock") = true;
        }
        self.apply(ConversationObservation::Status(
            crate::observation::status::status_view(&(observation.clone())),
        ));
    }

    fn observe_publication_opened(
        &self,
        attempt_id: &AttemptId,
        start: &crate::publication::PublicationStreamStart,
    ) {
        self.apply(ConversationObservation::PublicationOpened {
            attempt_id: attempt_id.clone(),
            start: start.clone(),
        });
    }

    fn observe_publication(
        &self,
        attempt_id: &AttemptId,
        frame: &crate::publication::PublicationFrame,
        _receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        self.apply(ConversationObservation::Publication {
            attempt_id: attempt_id.clone(),
            frame: frame.clone(),
        });
    }

    fn observe_publication_settled(
        &self,
        attempt_id: &AttemptId,
        _audit: &crate::publication::PublicationAudit,
        receipt: &crate::durable::PublicationAuditReceipt,
    ) {
        self.apply(ConversationObservation::AuditSettled {
            attempt_id: attempt_id.clone(),
            receipt: receipt.clone(),
        });
    }
}

/// The controlling half of the anchor race.
///
/// Everything here runs strictly inside the park: the Agent Loop is
/// blocked in `observe_status`, between the composition and its
/// publication, for the whole body of this function. Returns the tool
/// batch's own durable position and the later position the unrelated
/// inbound was accepted at.
async fn drive_anchor_race(
    observer: Arc<StatusParkObserver>,
    projection: Arc<Mutex<RuntimeClientProjection>>,
    mailbox: ConversationInboundMailbox,
    mut parked: tokio::sync::watch::Receiver<bool>,
    release: std::sync::mpsc::Sender<()>,
) -> (
    crate::durable::TranscriptCursor,
    crate::durable::TranscriptCursor,
) {
    parked
        .wait_for(|reached| *reached)
        .await
        .expect("the status park stays open");
    let batch_cursor = observer.tool_batch_cursor();

    // An independent durable boundary: a new inbound message is accepted,
    // and the durable transcript advances past the batch.
    mailbox
        .enqueue(UserMessageBlock {
            id: MessageId::new("unrelated-inbound"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "and another thing".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(chrono::Utc::now()),
        })
        .expect("durable inbound acceptance");
    let batch = mailbox
        .select_pending_batch()
        .expect("select pending")
        .expect("the accepted inbound is pending");
    let item = batch
        .items()
        .iter()
        .find(|item| item.message().id == MessageId::new("unrelated-inbound"))
        .expect("the accepted inbound is in the pending batch")
        .clone();
    let inbound_cursor = item
        .transcript_cursor()
        .expect("a visible inbound message is a durable transcript item");
    assert!(
        inbound_cursor > batch_cursor,
        "the unrelated inbound is durably later than the tool batch"
    );

    // Fold the acceptance into the Runtime Client *before* the status.
    projection
        .lock()
        .expect("projection lock")
        .apply(ConversationObservation::InboundEnqueued(item));
    let (mid_flight, _) = projection
        .lock()
        .expect("projection lock")
        .snapshot()
        .expect("snapshot");
    assert!(
        mid_flight.statuses.is_empty(),
        "the parked composition has not been published yet"
    );
    assert_eq!(
        mid_flight
            .inbound
            .pending
            .last()
            .expect("the unrelated inbound is folded")
            .message
            .id,
        MessageId::new("unrelated-inbound"),
        "the projection has already folded the later durable position"
    );

    release.send(()).expect("release the parked status");
    (batch_cursor, inbound_cursor)
}

/// **The `PostToolBatch` placement invariant.**
///
/// A `PostToolBatch` Agent Status is anchored to the durable transcript
/// frontier of the tool batch that caused the composition. Later
/// unrelated durable transcript activity cannot move that anchor.
///
/// The interleaving is forced, not hoped for:
///
/// ```text
/// 1. canonical ToolResult batch commits at cursor N
/// 2. the PostToolBatch opportunity freezes N
/// 3. the loop PARKS in observe_status, before the projection sees it
/// 4. an unrelated inbound is durably accepted at cursor N+1
/// 5. its InboundEnqueued observation is folded
/// 6. the park is RELEASED and the status is folded
/// 7. the published anchor is still N
/// ```
///
/// Step 5 is asserted before step 6 runs, so the projection provably held
/// the later position when it folded the composition. There are no
/// sleeps and no timing assumptions anywhere: every ordering above is a
/// rendezvous.
/// The scripted provider of the anchor race: one turn that calls `todo`,
/// then plain answers.
///
/// The tool is `todo` on purpose. The Todo module is the one status module
/// eligible for a `PostToolBatch` opportunity, so this batch both settles
/// a canonical `ToolResult` batch and makes the next primary step compose
/// a `PostToolBatch`-only status.
fn anchor_race_model() -> Arc<FakeModel> {
    let call = crate::tools::types::ToolCall {
        id: ToolCallId::new("call-anchor-1"),
        tool_id: ToolId::new(crate::tools::todo::TODO_TOOL_ID),
        name: "todo".to_owned(),
        arguments: serde_json::json!({ "action": "create", "subject": "Anchor the status" }),
    };
    let arguments_json = serde_json::to_string(&call.arguments).expect("serialize todo arguments");
    let stop_turn = || {
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]
    };
    Arc::new(FakeModel::new(vec![
        vec![
            FakeStep::Emit(ModelEvent::Started),
            FakeStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            }),
            FakeStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                arguments_delta: arguments_json,
            }),
            FakeStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call,
            }),
            FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ],
        stop_turn(),
        stop_turn(),
    ]))
}

/// A context runtime with a real Agent Status engine and no compaction
/// pressure, so the only status decision under test is placement.
fn anchor_race_context_runtime() -> ContextRuntime {
    ContextRuntime::with_scripted_summarizer(
        ContextEngine::new(
            crate::context::ContextConfig {
                context_window_tokens: 200_000,
                reserve_tokens: 0,
                keep_recent_tokens: 0,
            },
            Arc::new(ScriptedEstimator::new(1, 0, 0)),
        )
        .expect("valid test context engine"),
        Arc::new(FakeContextSummarizer::new(Vec::new())),
        Some(AgentStatusEngine::default()),
        CompactionBudgets::new(1, 1, 1_000_000),
    )
}

/// The attempt request of the anchor race: a pure continuation, so the
/// only status opportunity this attempt can produce is `PostToolBatch`.
fn anchor_race_request(
    conversation_id: ConversationId,
    model: Arc<FakeModel>,
) -> AgentExecutionRequest {
    let adapter: Arc<dyn ModelAdapter> = model;
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id,
        attempt_id: AttemptId::new("attempt-anchor"),
        conversation: ConversationState::from_messages(vec![MessageBlock::User(
            UserMessageBlock {
                id: MessageId::new("user-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "track the work".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            },
        )])
        .expect("bootstrap conversation"),
        initial_turn_trigger: InitialTurnTrigger::Continuation,
        model: crate::scripted_suites::support::attempt_model_with_window(
            adapter,
            "status-anchor",
            200_000,
            64,
        ),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_later_inbound_cannot_relocate_a_prepared_post_tool_batch_status() {
    let fixture = crate::scripted_suites::common::native_fixture();
    let capability = crate::scripted_suites::common::capability_lease(
        fixture.ordinary_registry.clone(),
        &fixture.runtime,
    )
    .await;
    let request = anchor_race_request(
        fixture.runtime.conversation_id().clone(),
        anchor_race_model(),
    );
    let projection = Arc::new(Mutex::new(projection()));
    let (observer, parked, release) = StatusParkObserver::install(projection.clone());

    let controller = tokio::spawn(drive_anchor_race(
        observer.clone(),
        projection.clone(),
        fixture.mailbox.clone(),
        parked,
        release,
    ));

    let cancellation = crate::agent::AgentCancellation::new(CancellationReason::UserRequested);
    let mut execution = AgentExecution::new(
        request,
        capability.into_lease(),
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        anchor_race_context_runtime(),
        &fixture.runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    execution.observe(observer.as_ref());
    let _result = execution.run().await;
    let (batch_cursor, inbound_cursor) = controller.await.expect("controller task");

    let (snapshot, _) = projection
        .lock()
        .expect("projection lock")
        .snapshot()
        .expect("snapshot");
    let status = snapshot
        .statuses
        .iter()
        .find(|status| status.opportunities.post_tool_batch.is_some())
        .expect("the attempt composed a PostToolBatch status");
    assert_eq!(
        published_anchor(status),
        Some(RuntimeClientTranscriptCursor::from(batch_cursor)),
        "the status keeps the position its own tool batch froze"
    );
    assert_ne!(
        published_anchor(status),
        Some(RuntimeClientTranscriptCursor::from(inbound_cursor)),
        "an unrelated inbound accepted before the fold cannot relocate the status"
    );
}

#[test]
fn agent_status_opportunity_view_allows_absent_fresh_inbound() {
    let observation = AgentStatusObservation {
        attempt_id: attempt(),
        turn: 1,
        status_message_id: MessageId::new("status-1"),
        opportunities: ContributionOpportunities::default(),
        post_tool_batch_anchor: None,
        status: AgentStatus {
            generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
            sections: Vec::new(),
        },
    };

    let view = crate::observation::status::status_view(&observation);
    assert!(view.opportunities.fresh_inbound.is_none());

    let encoded = serde_json::to_value(&view.opportunities).expect("serialize opportunity");
    assert_eq!(encoded, serde_json::json!({}));
    let decoded: AgentStatusOpportunityView =
        serde_json::from_value(encoded).expect("deserialize opportunity");
    assert_eq!(decoded, view.opportunities);
}

#[test]
fn agent_status_opportunity_view_publishes_one_combined_observation() {
    let status_message_id = MessageId::new("status-combined");
    let observation = AgentStatusObservation {
        attempt_id: attempt(),
        turn: 2,
        status_message_id: status_message_id.clone(),
        opportunities: ContributionOpportunities {
            fresh_inbound: Some(FreshInboundOpportunity {
                target_message_id: MessageId::new("fresh-inbound"),
            }),
            post_tool_batch: Some(PostToolBatchOpportunity),
        },
        post_tool_batch_anchor: Some(crate::durable::TranscriptCursor::new(6)),
        status: AgentStatus {
            generated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
            sections: Vec::new(),
        },
    };

    let view = crate::observation::status::status_view(&observation);
    assert_eq!(view.status_message_id, Some(status_message_id));
    assert_eq!(
        view.opportunities
            .fresh_inbound
            .as_ref()
            .expect("FreshInbound view")
            .target_message_id
            .clone(),
        Some(MessageId::new("fresh-inbound"))
    );
    assert_eq!(
        published_anchor(&view),
        Some(RuntimeClientTranscriptCursor::new(6)),
        "the PostToolBatch half carries the placement its batch froze"
    );
    let encoded = serde_json::to_value(&view).expect("serialize combined status view");
    assert_eq!(
        encoded["opportunities"]["post_tool_batch"],
        serde_json::json!({ "transcript_anchor": 6 })
    );
    assert!(
        encoded.get("transcript_anchor").is_none(),
        "placement lives on the opportunity that froze it, never on the view itself"
    );
    assert_eq!(
        encoded["status_message_id"],
        serde_json::json!("status-combined")
    );
}

#[test]
fn todo_status_section_round_trips_the_wire_shape() {
    let wire = serde_json::json!({
        "type": "todo",
        "current": {
            "id": 7,
            "subject": "Review the boundary",
            "active_form": "Reviewing the boundary",
            "status": "in_progress",
            "blocked": false
        },
        "tasks": [{
            "id": 8,
            "subject": "Write the regression",
            "status": "pending",
            "blocked": true
        }],
        "active_count": 2,
        "blocked_count": 1,
        "completed_count": 3,
        "deleted_count": 1,
        "omitted_count": 0
    });
    let section: RuntimeClientStatusSection =
        serde_json::from_value(wire.clone()).expect("decode the Runtime Client Todo section");
    assert_eq!(
        serde_json::to_value(section).expect("encode the Runtime Client Todo section"),
        wire
    );
}

fn single_fact(
    event: &crate::runtime_client::types::RuntimeClientProtocolEvent,
) -> &RuntimeClientEvent {
    let RuntimeClientEvent::ProjectionTransition { events, metadata } = &event.event else {
        panic!("complete native transition: {:?}", event.event);
    };
    assert_eq!(metadata.read_cut.revision, event.cursor.get());
    let [fact] = events.as_slice() else {
        panic!("one complete fact at this cursor: {events:?}");
    };
    fact
}

fn collect(
    projection: &mut RuntimeClientProjection,
    after: RuntimeClientCursor,
) -> Vec<crate::runtime_client::types::RuntimeClientProtocolEvent> {
    let (id, _notify) = projection.subscribe(after).expect("serviceable cursor");
    let mut events = Vec::new();
    loop {
        match projection.poll_subscriber(id) {
            SubscriberPoll::Event(event) => events.push(event),
            SubscriberPoll::Pending => break,
            other => panic!("unexpected subscriber poll: {other:?}"),
        }
    }
    projection.remove_subscriber(id);
    events
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompactionOrderFact {
    /// The typed checkpoint and local completion fact committed atomically.
    RuntimeCompactionCompleted,
    ClientContextCompacted,
    ClientAttemptSettled,
}

struct EventPathObserver {
    store: Arc<dyn crate::durable::ConversationStore>,
    projection: Mutex<RuntimeClientProjection>,
    subscriber: u64,
    facts: Arc<Mutex<Vec<CompactionOrderFact>>>,
}

impl EventPathObserver {
    fn new(
        initial_messages: Vec<MessageBlock>,
        facts: Arc<Mutex<Vec<CompactionOrderFact>>>,
        store: Arc<dyn crate::durable::ConversationStore>,
    ) -> Self {
        let mut projection = RuntimeClientProjection::new(
            ConversationId::new("conv_d8b669b8-1b38-797b-8db2-c56430c1741d"),
            initial_messages,
            crate::observation::capability_source::CapabilityView {
                configured_tools: Vec::new(),
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: Vec::new(),
                available_tools: Vec::new(),
                skills: Vec::new(),
                sources: Vec::new(),
            },
            Some(model_view()),
            64,
        );
        projection
            .initialize_descriptor_heads(store.as_ref(), store.observation_read_cut().unwrap())
            .unwrap();
        let (subscriber, _notify) = projection
            .subscribe(RuntimeClientCursor::new(0))
            .expect("subscribe projection observer");
        Self {
            store,
            projection: Mutex::new(projection),
            subscriber,
            facts,
        }
    }

    fn release_native_batch(&self, receipt: &crate::durable::observation::NativeObservationBatch) {
        if let Some(canonical) = &receipt.canonical {
            self.store
                .mark_canonical_transcript_ready(canonical.first_position, canonical.last_position)
                .unwrap();
        }
        if let Some((first, last)) = receipt.journal_range {
            for sequence in first..=last {
                if let Some(cursor) = self
                    .store
                    .event_transcript_cursor_at_sequence(sequence)
                    .unwrap()
                {
                    self.store
                        .mark_observation_transcript_ready(cursor.get())
                        .unwrap();
                }
            }
            self.store.mark_observation_direct(first, last).unwrap();
        }
        let ready = self.store.transcript_source_ready().unwrap();
        let mut projection = self.projection.lock().unwrap();
        projection.apply(ConversationObservation::TranscriptSourceReady(ready));
        self.drain_client_events(&mut projection);
    }

    fn drain_client_events(&self, projection: &mut RuntimeClientProjection) {
        loop {
            match projection.poll_subscriber(self.subscriber) {
                SubscriberPoll::Event(event) => {
                    for fact in event.event.facts() {
                        match fact {
                            RuntimeClientEvent::ContextCompacted { .. } => self
                                .facts
                                .lock()
                                .expect("order facts lock")
                                .push(CompactionOrderFact::ClientContextCompacted),
                            RuntimeClientEvent::AttemptSettled { .. } => self
                                .facts
                                .lock()
                                .expect("order facts lock")
                                .push(CompactionOrderFact::ClientAttemptSettled),
                            _ => {}
                        }
                    }
                }
                SubscriberPoll::Pending => return,
                other => panic!("projection observer lost its subscription: {other:?}"),
            }
        }
    }

    fn snapshot(&self) -> crate::runtime_client::snapshot::RuntimeClientSnapshot {
        self.projection
            .lock()
            .expect("projection lock")
            .snapshot()
            .expect("projection snapshot")
            .0
    }
}

impl AgentExecutionObserver for EventPathObserver {
    fn observe_event(
        &self,
        _attempt_id: &AttemptId,
        event: &RuntimeEvent,
        journal_sequence: u64,
        _tool_occurrence: Option<&crate::events::types::ToolObservationOccurrence>,
        receipt: Option<&crate::durable::observation::NativeObservationBatch>,
    ) {
        if matches!(event, RuntimeEvent::CompactionCompleted { .. }) {
            self.facts
                .lock()
                .expect("order facts lock")
                .push(CompactionOrderFact::RuntimeCompactionCompleted);
        }
        let receipt = receipt.expect("the actual native Journal receipt");
        if receipt
            .journal_range
            .is_some_and(|(_, last)| journal_sequence < last)
        {
            return;
        }
        {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply_native_batch(receipt);
            self.drain_client_events(&mut projection);
        }
        self.release_native_batch(receipt);
    }

    fn observe_committed(
        &self,
        attempt_id: &AttemptId,
        _block: &MessageBlock,
        receipt: &crate::durable::TranscriptCommitReceipt,
    ) {
        let mut projection = self.projection.lock().expect("projection lock");
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: Some(attempt_id.clone()),
            receipt: receipt.clone(),
        });
        self.drain_client_events(&mut projection);
    }

    fn observe_status(
        &self,
        observation: &AgentStatusObservation,
        source: Option<&crate::durable::StatusObservationSource>,
    ) {
        let mut projection = self.projection.lock().expect("projection lock");
        projection.apply(ConversationObservation::Status(
            crate::observation::status::prepared_status_view(observation, source),
        ));
        self.drain_client_events(&mut projection);
    }

    fn observe_publication_opened(
        &self,
        attempt_id: &AttemptId,
        start: &crate::publication::PublicationStreamStart,
    ) {
        let mut projection = self.projection.lock().expect("projection lock");
        projection.apply(ConversationObservation::PublicationOpened {
            attempt_id: attempt_id.clone(),
            start: start.clone(),
        });
        self.drain_client_events(&mut projection);
    }

    fn observe_publication(
        &self,
        attempt_id: &AttemptId,
        frame: &crate::publication::PublicationFrame,
        _receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        let mut projection = self.projection.lock().expect("projection lock");
        projection.apply(ConversationObservation::Publication {
            attempt_id: attempt_id.clone(),
            frame: frame.clone(),
        });
        self.drain_client_events(&mut projection);
    }

    fn observe_publication_settled(
        &self,
        attempt_id: &AttemptId,
        _audit: &crate::publication::PublicationAudit,
        receipt: &crate::durable::PublicationAuditReceipt,
    ) {
        {
            let mut projection = self.projection.lock().expect("projection lock");
            projection.apply(ConversationObservation::AuditSettled {
                attempt_id: attempt_id.clone(),
                receipt: receipt.clone(),
            });
            self.drain_client_events(&mut projection);
        }
        self.store
            .mark_observation_transcript_ready(receipt.transcript_cursor.get())
            .unwrap();
        let ready = self.store.transcript_source_ready().unwrap();
        let mut projection = self.projection.lock().unwrap();
        projection.apply(ConversationObservation::TranscriptSourceReady(ready));
        self.drain_client_events(&mut projection);
    }
}

fn compactable_user(id: &str) -> MessageBlock {
    MessageBlock::User(UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: "history".to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: None,
    })
}

async fn run_compaction_order_case(
    fail_summary: bool,
) -> (
    crate::scripted_suites::common::DurableExecutionAudit,
    Arc<EventPathObserver>,
    Arc<Mutex<Vec<CompactionOrderFact>>>,
) {
    let facts = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(FakeModel::new(vec![vec![
        FakeStep::Emit(ModelEvent::Started),
        FakeStep::Emit(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: "done".to_owned(),
        }),
        FakeStep::Emit(ModelEvent::Completed {
            finish_reason: ModelFinishReason::Stop,
            usage: None,
        }),
    ]]));
    let adapter: Arc<dyn ModelAdapter> = model.clone();
    let model_snapshot = crate::scripted_suites::support::attempt_model_with_window(
        adapter,
        "projection-order",
        200,
        1,
    );
    let initial_messages = vec![compactable_user("old-1"), compactable_user("old-2")];
    let engine = ContextEngine::new(
        crate::context::ContextConfig {
            context_window_tokens: 200,
            reserve_tokens: 0,
            keep_recent_tokens: 0,
        },
        Arc::new(ScriptedEstimator::new(120, 0, 0)),
    )
    .expect("valid test context engine");
    let summary_step = if fail_summary {
        FakeSummaryStep::Fail(ContextError::new(
            ContextErrorKind::SummaryFailed,
            "scripted summary failure",
        ))
    } else {
        FakeSummaryStep::Return("summary".to_owned())
    };
    let runtime = ContextRuntime::with_scripted_summarizer(
        engine,
        Arc::new(FakeContextSummarizer::new(vec![summary_step])),
        Some(AgentStatusEngine::default()),
        CompactionBudgets::new(1, 1, 1_000_000),
    );
    let tool_runtime =
        crate::scripted_suites::common::tool_runtime("conv_d8b669b8-1b38-797b-8db2-c56430c1741d");
    let store = tool_runtime.durable_store();
    let capability =
        crate::scripted_suites::common::capability_lease(ToolRegistry::new(), &tool_runtime).await;
    let request = AgentExecutionRequest {
        agent_id: AgentId::new("agent-a"),
        conversation_id: ConversationId::new("conv_d8b669b8-1b38-797b-8db2-c56430c1741d"),
        attempt_id: AttemptId::new("attempt-order"),
        conversation: ConversationState::from_messages(initial_messages.clone())
            .expect("bootstrap conversation"),
        initial_turn_trigger: InitialTurnTrigger::Continuation,
        model: model_snapshot,
    };
    let cancellation = crate::agent::AgentCancellation::new(CancellationReason::UserRequested);
    let mut execution = AgentExecution::new(
        request,
        capability.into_lease(),
        &cancellation,
        crate::scripted_suites::support::default_execution_policy(),
        runtime,
        &tool_runtime,
        crate::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime");
    let observer = Arc::new(EventPathObserver::new(
        initial_messages,
        facts.clone(),
        store.clone(),
    ));
    execution.observe(observer.as_ref());
    let result = execution.run().await;
    let result = crate::scripted_suites::common::durable_agent_result(result, store.as_ref());
    (result, observer, facts)
}

/// A successful committed compaction has one ordered fact path: the
/// immutable checkpoint activation/receipt, the actual local completion event, the
/// Runtime Client event, and terminal settlement.
///
/// The ordering is the client-visible invariant: no `ContextCompacted`
/// may imply success before immutable content and the durable Projection Head exist.
#[tokio::test]
async fn committed_compaction_order_is_canonical_and_terminal_last() {
    let (result, observer, facts) = run_compaction_order_case(false).await;
    assert_eq!(
        *facts.lock().expect("order facts lock"),
        vec![
            CompactionOrderFact::RuntimeCompactionCompleted,
            CompactionOrderFact::ClientContextCompacted,
            CompactionOrderFact::ClientAttemptSettled,
        ]
    );
    assert!(matches!(
        result.event_history.last(),
        Some(RuntimeEvent::AttemptCompleted { .. })
    ));
    assert_eq!(result.conversation.surface().compaction_generation(), 1);
    let snapshot = observer.snapshot();
    assert_eq!(snapshot.context.compaction_count, 1);
    let latest = snapshot
        .context
        .latest_compaction
        .expect("latest compaction");
    assert_eq!(latest.generation, 1);
    assert_eq!(
        latest.activation_id,
        result
            .event_history
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::CompactionCompleted { activation_id, .. } => Some(*activation_id),
                _ => None,
            })
            .expect("one compaction completion"),
        "the client view carries the revision the rewrite established"
    );
    assert_eq!(
        snapshot.messages.len(),
        result.canonical_history.len(),
        "client transcript contains only canonical execution facts"
    );
}

/// A failed compaction publishes its live start/failure lifecycle and
/// terminal settlement, but never commits a canonical summary and never
/// emits canonical or client completion.
#[tokio::test]
async fn failed_compaction_has_no_summary_commit_or_client_event() {
    let (result, observer, facts) = run_compaction_order_case(true).await;
    assert_eq!(
        *facts.lock().expect("order facts lock"),
        vec![CompactionOrderFact::ClientAttemptSettled]
    );
    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionStarted { .. }))
    );
    assert!(
        result
            .event_history
            .iter()
            .any(|event| matches!(event, RuntimeEvent::CompactionFailed { .. }))
    );
    assert!(
        result
            .event_history
            .iter()
            .all(|event| { !matches!(event, RuntimeEvent::CompactionCompleted { .. }) })
    );
    assert!(matches!(
        result.event_history.last(),
        Some(RuntimeEvent::AttemptFailed { .. })
    ));
    assert_eq!(result.conversation.surface().compaction_generation(), 0);
    assert!(
        result.projections.is_empty(),
        "failed maintenance installs no projection"
    );
    let snapshot = observer.snapshot();
    assert!(!snapshot.context.compaction_in_progress);
    assert_eq!(snapshot.context.compaction_count, 0);
    assert!(snapshot.context.latest_compaction.is_none());
}

/// Canonical completion events advance the external context view exactly
/// once per committed generation and preserve token-measurement
/// provenance.
#[test]
fn committed_compactions_are_visible_without_exposing_summary_content() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::CompactionCompleted {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
            generation: 1,
            checkpoint_id: crate::runtime::identity::CheckpointId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-1",
            ),
            activation_id: crate::context::checkpoint::ProjectionActivationId(3),
            tokens_before: TokenMeasurement {
                input_tokens: 4800,
                source: TokenMeasurementSource::ProviderReported,
            },
            estimated_tokens_after: 1700,
            occupancy: None,
        },
    );
    apply_event(
        &mut projection,
        RuntimeEvent::CompactionCompleted {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
            generation: 2,
            checkpoint_id: crate::runtime::identity::CheckpointId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-2",
            ),
            activation_id: crate::context::checkpoint::ProjectionActivationId(6),
            tokens_before: TokenMeasurement {
                input_tokens: 4700,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 1800,
            occupancy: None,
        },
    );

    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.messages, Vec::new());
    assert_eq!(snapshot.context.compaction_count, 2);
    let latest = snapshot
        .context
        .latest_compaction
        .expect("latest compaction");
    assert_eq!(latest.generation, 2);
    assert_eq!(
        latest.tokens_before.source,
        TokenMeasurementSource::Estimated
    );
    assert_eq!(latest.estimated_tokens_after, 1800);

    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    assert_eq!(events.len(), 2);
    assert!(events[0].event.facts().iter().any(
        |fact| matches!(fact, RuntimeClientEvent::ContextCompacted { attempt_id, context }
            if attempt_id.as_ref() == Some(&attempt())
                && context.compaction_count == 1
                && context.latest_compaction.as_ref().is_some_and(|view| view.generation == 1))
    ));
    assert!(events[1].event.facts().iter().any(
        |fact| matches!(fact, RuntimeClientEvent::ContextCompacted { context, .. }
            if context.compaction_count == 2
                && context.latest_compaction.as_ref().is_some_and(|view| view.generation == 2))
    ));
    assert_eq!(cursor, RuntimeClientCursor::new(2));
}

fn success_result() -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Success,
        content: Vec::new(),
        duration_ms: 1,
        exit_code: Some(0),
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

fn cancelled_result(
    reason: CancellationReason,
    phase: ToolCancellationPhase,
) -> ToolExecutionResult {
    ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Cancelled { reason, phase },
        content: Vec::new(),
        duration_ms: 1,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    }
}

fn apply_assembled_call(projection: &mut RuntimeClientProjection, call: &ToolCall) {
    apply_frame(
        projection,
        0,
        PublicationPayload::ProposedToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
            },
        },
    );
    apply_frame(
        projection,
        1,
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index: ContentBlockIndex::new(0),
            call_id: call.id.clone(),
            suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
        },
    );
    apply_frame(
        projection,
        2,
        PublicationPayload::ProposedToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: call.clone(),
        },
    );
}

#[test]
fn journal_batch_invalidates_trace_even_when_session_events_publish() {
    use crate::events::types::RuntimeEventEnvelope;
    use crate::runtime::identity::EventId;
    use crate::runtime::observation::PendingObservations;
    let mut projection = projection();
    let queue = PendingObservations::new();
    let facts = [
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
        RuntimeEvent::TurnStarted,
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call"),
            tool_id: ToolId::new("tool"),
        },
    ];
    for (index, event) in facts.into_iter().enumerate() {
        let sequence = index as u64 + 1;
        queue.committed_fixture(Some(vec![RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
            event_id: EventId::new(format!("trace-{sequence}")),
            sequence,
            conversation_id: projection.snapshot_ref().conversation_id.clone(),
            attempt_id: Some(attempt()),
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            event: event.clone(),
        }]));
        queue.push(ConversationObservation::Published {
            journal_sequence: sequence,
            observation: Box::new(event_observation(event)),
        });
    }
    let batches = queue.drain();
    assert!(matches!(
        batches.as_slice(),
        [ConversationObservation::JournalBatch {
            through: Some(3),
            ..
        }]
    ));
    for batch in batches {
        projection.apply(batch);
    }
    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    assert!(events.iter().any(|e| {
        e.event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::ToolExecutionStarted { .. }))
    }));
    assert!(events.iter().any(|e| {
        e.event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
    }));
    assert!(events.iter().any(|e| {
        e.event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptTurnUpdated { .. }))
    }));
    assert_eq!(
        events
            .iter()
            .filter(|e| e
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::TraceChanged)))
            .count(),
        1
    );
}

#[test]
fn failed_journal_batch_wakes_subscribers_without_inventing_a_trace_cut() {
    use futures_util::FutureExt as _;
    let mut projection = projection();
    let (subscriber, wake) = projection.subscribe(RuntimeClientCursor::new(0)).unwrap();
    assert!(wake.notified().now_or_never().is_none());
    projection.apply(ConversationObservation::JournalBatch {
        through: None,
        observations: Vec::new(),
    });
    assert!(wake.notified().now_or_never().is_some());
    assert_eq!(
        projection.poll_subscriber(subscriber),
        SubscriberPoll::Unavailable {
            message: "native Journal observation source unavailable".into(),
        }
    );
    assert!(matches!(
        projection.subscribe(RuntimeClientCursor::new(0)),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert!(matches!(
        projection.validate_published_source(),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert_eq!(projection.cursor(), RuntimeClientCursor::new(0));
}

#[test]
fn trace_progress_retains_dependency_revision_but_retires_the_older_journal_cut() {
    let mut projection = projection();
    projection.apply(event_observation(RuntimeEvent::TurnStarted));
    let cut = projection.read_domain_cut().unwrap();
    projection.apply(ConversationObservation::JournalBatch {
        through: Some(9),
        observations: vec![event_observation(RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("trace-only-call"),
            tool_id: ToolId::new("tool"),
        })],
    });
    let current = projection.read_domain_cut().unwrap();
    assert_eq!(current.revision, cut.revision);
    assert_eq!(current.through, 9);
    assert_eq!(projection.snapshot_cut().unwrap().2, 9);
    assert!(!projection.install_read_domains(
        &cut,
        Ok(super::ReadDomainValues {
            projection_head: None,
            transcript: crate::observation::transcript::RuntimeClientTranscriptPage::default(),
            occupancy: None,
            trace: crate::observation::trace::TracePage::default(),
            trace_total: 0,
        }),
        true
    ));
}

#[test]
fn t14_t19_worker_read_fence_keeps_one_native_cut_before_publication() {
    let (mut projection, store) = initialized_source_projection();
    let initial = projection.checkpoint_candidate().unwrap();
    let first = store
        .append_event(source_tool_event(
            &store,
            RuntimeEvent::TurnCompleted,
            "worker-cut-first",
        ))
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(first.observation.as_deref().unwrap());
    });
    store
        .mark_observation_direct(first.sequence, first.sequence)
        .unwrap();
    assert_eq!(
        projection.checkpoint_candidate().unwrap(),
        initial,
        "a trace-only working update does not expose an unencoded published cut"
    );
    let old_fence = projection.read_domain_cut().unwrap();
    assert_eq!(old_fence.through, first.sequence);
    assert_eq!(
        old_fence.source_cut,
        Some(store.observation_read_cut().unwrap())
    );
    let old_read = old_fence.read(&store).unwrap();
    assert_eq!(
        old_read
            .transcript
            .source_cut
            .as_ref()
            .unwrap()
            .read
            .journal,
        old_fence.through
    );
    let mut second_event =
        source_tool_event(&store, RuntimeEvent::TurnCompleted, "worker-cut-second");
    second_event.turn_id = Some(TurnId::new("2"));
    let second = store.append_event(second_event).unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(second.observation.as_deref().unwrap());
    });
    store
        .mark_observation_direct(second.sequence, second.sequence)
        .unwrap();
    let current = projection.read_domain_cut().unwrap();
    assert_eq!(current.through, second.sequence);
    assert_eq!(
        current.source_cut,
        Some(store.observation_read_cut().unwrap())
    );
    assert!(
        !projection.install_read_domains(&old_fence, Ok(old_read), true),
        "a coherent older worker read still cannot install at a newer native fence"
    );
    assert_eq!(projection.checkpoint_candidate().unwrap(), initial);
    let read = current.read(&store).unwrap();
    publish_source_fact(&mut projection, |projection| {
        assert!(projection.install_read_domains(&current, Ok(read), true));
    });
    let published = projection.checkpoint_candidate().unwrap();
    assert_eq!(published.cursor.get(), initial.cursor.get() + 1);
    assert_eq!(published.read_cut.through, second.sequence);
    assert_eq!(published.read_cut.source_cut, current.source_cut);
    assert_eq!(
        published
            .snapshot
            .transcript
            .source_cut
            .as_ref()
            .unwrap()
            .read
            .journal,
        second.sequence
    );
}

#[test]
fn read_failure_requires_successful_source_read_at_the_current_dependency_revision() {
    let mut projection = projection();
    projection.apply(event_observation(RuntimeEvent::TurnStarted));
    let failed = projection.read_domain_cut().unwrap();
    assert!(projection.install_read_domains(
        &failed,
        Err(crate::durable::ConversationStoreError::InvalidReference(
            "transient".into()
        )),
        true
    ));
    assert!(projection.read_failure.is_some());
    assert!(projection.read_domain_cut().is_none());
    let cursor = projection.cursor();
    assert_eq!(
        projection.request_failed_read_repair(),
        Some((failed.clone(), true))
    );
    assert_eq!(
        projection.request_failed_read_repair(),
        Some((failed.clone(), false))
    );
    assert_eq!(
        projection.cursor(),
        cursor,
        "concurrent requests coalesce at one native fence"
    );
    assert!(projection.install_read_domains(
        &failed,
        Err(crate::durable::ConversationStoreError::InvalidReference(
            "still unavailable".into()
        )),
        true,
    ));
    projection.apply(ConversationObservation::JournalSourceReady);
    assert!(
        projection.read_domain_cut().is_none(),
        "a late wake cannot restart a completed failed read"
    );
    assert_eq!(projection.cursor(), cursor);
    projection.apply(event_observation(RuntimeEvent::TurnStarted));
    let next = projection.read_domain_cut().unwrap();
    assert_ne!(next, failed);
    assert!(
        projection.read_failure.is_some(),
        "a new dependency is not proof that its Source read succeeded"
    );
    let domains = super::ReadDomainValues {
        projection_head: projection.snapshot.context.projection_head.clone(),
        transcript: projection.snapshot.transcript.clone(),
        occupancy: None,
        trace: crate::observation::trace::TracePage::default(),
        trace_total: 0,
    };
    assert!(!projection.install_read_domains(&failed, Ok(domains.clone()), true));
    assert_eq!(projection.read_domain_cut(), Some(next.clone()));
    assert!(projection.install_read_domains(&next, Ok(domains), true));
    assert!(projection.read_failure.is_none());
    assert!(projection.read_domain_cut().is_none());
    // An older in-flight read for the same fence cannot poison a cut which
    // another successful owner has already established.
    assert!(!projection.install_read_domains(
        &next,
        Err(crate::durable::ConversationStoreError::InvalidReference(
            "late failure".into()
        )),
        true
    ));
    assert!(projection.snapshot().is_ok());
}

#[test]
fn durable_read_cut_retires_stale_results_without_overwriting_semantics() {
    let mut projection = projection();
    projection.apply(event_observation(RuntimeEvent::TurnStarted));
    let old = projection.read_domain_cut().unwrap();
    projection.apply(ConversationObservation::Shutdown);
    assert_eq!(
        projection.read_domain_cut(),
        Some(old.clone()),
        "shutdown does not invalidate durable domains"
    );
    projection.apply(event_observation(RuntimeEvent::TurnStarted));
    assert!(!projection.install_read_domains(
        &old,
        Ok(super::ReadDomainValues {
            projection_head: None,
            transcript: crate::observation::transcript::RuntimeClientTranscriptPage::default(),
            occupancy: None,
            trace: crate::observation::trace::TracePage::default(),
            trace_total: 0,
        }),
        true
    ));
    assert!(projection.snapshot_ref().shutting_down);
    assert!(projection.read_domain_cut().is_some());
}

/// The receipt fold is the CPU linearization point. A late audit retains
/// its positive fact but cannot clear a replacement with the same message.
#[test]
fn t14_t19_audit_receipt_fences_exact_stream_and_preserves_deferred_settlement_head() {
    let mut projection = projection();
    projection.apply(event_observation(RuntimeEvent::AttemptStarted {
        attempt_id: attempt(),
    }));
    apply_publication_open(&mut projection);
    let original = stream_start();
    let store = crate::durable::SqliteConversationStore::in_memory(
        projection.snapshot.conversation_id.clone(),
    )
    .unwrap();
    store.initialize(&[]).unwrap();
    let cut = store.observation_read_cut().unwrap();
    projection.source_cut = Some(cut.clone());
    let receipt = |start: &crate::publication::PublicationStreamStart, position: u64| {
        crate::durable::PublicationAuditReceipt {
            ready_through: Some(0),
            ready_t_through: Some(0),
            summary: crate::durable::PublicationAuditSummary {
                stream_id: Some(start.stream_id.clone()),
                message_id: Some(start.message_id.clone()),
                attempt_id: Some(start.attempt_id.clone()),
                turn_id: Some(start.turn_id.clone()),
                request_id: Some(start.request_id.clone()),
                kind: crate::publication::PublicationAuditKind::Incomplete,
                settled_at: chrono::Utc::now(),
                block_count: 73,
            },
            transcript_cursor: crate::durable::TranscriptCursor::new(position),
            read_cut: cut.clone(),
            header: crate::durable::observation::TranscriptHeader {
                tool_calls: Vec::new(),
                publication_settlement: None,
                cursor: crate::durable::TranscriptCursor::new(position),
                reference_kind: "publication_audit".into(),
                reference_id: format!("transcript:{position}"),
                message: None,
                display: None,
                content: Vec::new(),
                collections: Vec::new(),
                deferred: true,
            },
        }
    };
    let mut replacement = original.clone();
    replacement.stream_id =
        crate::runtime::identity::PublicationStreamId::new("replacement-same-message");
    replacement.request_id = RequestId::new("replacement-request");
    projection.apply(ConversationObservation::PublicationOpened {
        attempt_id: attempt(),
        start: replacement.clone(),
    });
    let execution_phase = projection.snapshot.attempt.as_ref().unwrap().phase.clone();
    let events = projection.fold_audit_receipt(attempt(), receipt(&original, 1));
    assert_eq!(projection.publication_start.as_ref(), Some(&replacement));
    assert!(
        projection
            .snapshot
            .attempt
            .as_ref()
            .unwrap()
            .in_flight
            .is_some()
    );
    assert!(
        matches!(&events[0],RuntimeClientEvent::AssistantPublicationDeferredSettled {settlement,settlement_head,..} if settlement.block_count==73 && settlement_head.len()==1)
    );
    let events = projection.fold_audit_receipt(attempt(), receipt(&replacement, 2));
    assert!(projection.publication_start.is_none());
    assert!(
        projection
            .snapshot
            .attempt
            .as_ref()
            .unwrap()
            .in_flight
            .is_none()
    );
    assert!(
        matches!(&events[0],RuntimeClientEvent::AssistantPublicationDeferredSettled {settlement_head,..} if settlement_head==&projection.snapshot.publication_settlements)
    );
    assert_eq!(projection.snapshot.publication_settlements.len(), 2);
    assert_eq!(
        projection.snapshot.attempt.as_ref().unwrap().phase,
        execution_phase,
        "Publication audit is not execution settlement"
    );
}

fn stream_start() -> crate::publication::PublicationStreamStart {
    crate::publication::PublicationStreamStart {
        stream_id: crate::runtime::identity::PublicationStreamId::new("attempt-1-pub-1"),
        attempt_id: attempt(),
        turn_id: crate::runtime::identity::TurnId::new("1"),
        request_id: RequestId::new("request:9:attempt-1:1:1:0"),
        message_id: MessageId::new("msg-1"),
    }
}

fn frame(sequence: u64, payload: PublicationPayload) -> PublicationFrame {
    PublicationFrame {
        stream_id: stream_start().stream_id,
        message_id: MessageId::new("msg-1"),
        sequence,
        payload,
    }
}

/// The representative attempt sequence: streaming publication, a tool
/// call, execution, and terminal settlement.
///
/// Streaming assembly arrives as durably committed publication frames
/// (Issue #108); only the surrounding execution facts are Event Journal
/// observations.
#[allow(clippy::too_many_lines)] // Representative event history for projection conformance.
fn representative_sequence() -> Vec<ConversationObservation> {
    let mut observations = vec![
        event_observation(RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        }),
        event_observation(RuntimeEvent::TurnStarted),
        event_observation(RuntimeEvent::ModelRequestStarted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: crate::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            model: "scripted".to_owned(),
        }),
        ConversationObservation::PublicationOpened {
            attempt_id: attempt(),
            start: stream_start(),
        },
    ];
    for (sequence, payload) in [
        PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: "hello ".to_owned(),
        },
        PublicationPayload::TextSuffix {
            block_index: ContentBlockIndex::new(0),
            suffix: "world".to_owned(),
        },
        PublicationPayload::ProposedToolCallStarted {
            block_index: ContentBlockIndex::new(1),
            call: ToolCallStart {
                id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
            },
        },
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index: ContentBlockIndex::new(1),
            call_id: ToolCallId::new("call_1"),
            suffix: "{}".to_owned(),
        },
        PublicationPayload::ProposedToolCallCompleted {
            block_index: ContentBlockIndex::new(1),
            call: ToolCall {
                id: ToolCallId::new("call_1"),
                tool_id: ToolId::new("tool-alpha"),
                name: "alpha".to_owned(),
                arguments: serde_json::json!({}),
            },
        },
    ]
    .into_iter()
    .enumerate()
    {
        observations.push(ConversationObservation::Publication {
            attempt_id: attempt(),
            frame: frame(sequence as u64, payload),
        });
    }
    observations.extend([
        event_observation(RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: attempt(),
                    turn: crate::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            finish_reason: ModelFinishReason::ToolCalls,
            usage: None,
            generation: None,
        }),
        event_observation(RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call_1"),
            tool_id: ToolId::new("tool-alpha"),
        }),
        event_observation(RuntimeEvent::ToolExecutionProgress {
            tool_call_id: ToolCallId::new("call_1"),
            tool_id: ToolId::new("tool-alpha"),
            execution_id: None,
            progress: ToolProgress {
                message: Some("half way".to_owned()),
                completed: Some(1.0),
                total: Some(2.0),
            },
        }),
        event_observation(RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new("call_1"),
            tool_id: ToolId::new("tool-alpha"),
            result: success_result(),
        }),
        event_observation(RuntimeEvent::TurnCompleted),
        event_observation(RuntimeEvent::AttemptCompleted {
            attempt_id: attempt(),
            finish_reason: ModelFinishReason::Stop,
        }),
    ]);
    observations
}

/// The projection is deterministic: applying the same representative
/// sequence twice produces identical event sequences and identical
/// snapshots.
#[test]
fn representative_sequence_projects_deterministically() {
    let mut first = projection();
    for observation in representative_sequence() {
        first.apply(observation);
    }
    let mut second = projection();
    for observation in representative_sequence() {
        second.apply(observation);
    }
    let first_events = collect(&mut first, RuntimeClientCursor::new(0));
    let second_events = collect(&mut second, RuntimeClientCursor::new(0));
    assert_eq!(first_events, second_events);
    assert!(!first_events.is_empty(), "the sequence publishes events");
    let (first_snapshot, first_cursor) = first.snapshot().expect("snapshot");
    let (second_snapshot, second_cursor) = second.snapshot().expect("snapshot");
    assert_eq!(first_snapshot, second_snapshot);
    assert_eq!(first_cursor, second_cursor);
}

/// Request payloads stay internal; invalidation and compaction lifecycle and
/// committed completion are projected as runtime-owned lifecycle facts.
#[test]
fn request_payloads_stay_internal_and_trace_is_owned_by_journal_batch() {
    let mut projection = projection();
    for event in [
        RuntimeEvent::ModelRequestStarted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: AttemptId::new("attempt-1"),
                    turn: crate::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            model: "m".to_owned(),
        },
        RuntimeEvent::ModelRequestFailed {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: attempt(),
                    turn: crate::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            error: ModelError {
                kind: ModelErrorKind::RateLimit,
                message: "retry".to_owned(),
                retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                retry_after_ms: Some(10),
                provider_code: Some("rate_limit_exceeded".to_owned()),
                context_overflow: None,
                malformed_tool_proposal: None,
                timeout_phase: None,
                generation: None,
            },
            usage: None,
            generation: None,
        },
        RuntimeEvent::ModelRetryScheduled {
            failed_request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            retry_number: 1,
            retry_delay_ms: Some(5),
        },
        RuntimeEvent::CompactionStarted {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
        },
        RuntimeEvent::CompactionCompleted {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
            generation: 1,
            checkpoint_id: crate::runtime::identity::CheckpointId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-1",
            ),
            activation_id: crate::context::checkpoint::ProjectionActivationId(3),
            tokens_before: TokenMeasurement {
                input_tokens: 1,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 1,
            occupancy: None,
        },
        RuntimeEvent::CompactionFailed {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
            error: "boom".to_owned(),
        },
        RuntimeEvent::TurnCompleted,
    ] {
        apply_event(&mut projection, event);
    }
    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    assert_eq!(events.len(), 3);
    assert!(
        events[0]
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::ContextCompactionStarted { .. }))
    );
    assert!(events[1].event.facts().iter().any(
        |fact| matches!(fact, RuntimeClientEvent::ContextCompacted { context, .. }
            if context.compaction_count == 1)
    ));
    assert!(events[2].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::ContextCompactionFailed { error, .. } if error == "boom")));
    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(3));
    assert!(!snapshot.context.compaction_in_progress);
    assert_eq!(snapshot.context.compaction_count, 1);
    assert!(snapshot.attempt.is_none());
}

#[test]
fn manual_compaction_projects_without_fabricating_an_attempt_identity() {
    let mut projection = projection();
    projection.apply(ConversationObservation::ManualCompactionEvent {
        request_id: Some("test-request".to_owned().try_into().unwrap()),
        event: RuntimeEvent::CompactionStarted {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
        },
    });
    let admitted = projection
        .snapshot_ref()
        .context
        .manual_compaction
        .as_ref()
        .unwrap();
    assert_eq!(
        admitted
            .request_id
            .as_ref()
            .map(crate::runtime::identity::ManualCompactionRequestId::as_str),
        Some("test-request")
    );
    assert!(!admitted.released);
    projection.apply(ConversationObservation::ManualCompactionEvent {
        request_id: Some("test-request".to_owned().try_into().unwrap()),
        event: RuntimeEvent::CompactionCompleted {
            operation_id: crate::runtime::identity::CompactionOperationId::new(
                "fixture-compaction",
            ),
            generation: 1,
            checkpoint_id: crate::runtime::identity::CheckpointId::new(
                "conv_36524fd8-f674-7fc2-8125-06d01fee0e18-summary-1",
            ),
            activation_id: crate::context::checkpoint::ProjectionActivationId(2),
            tokens_before: TokenMeasurement {
                input_tokens: 8_400,
                source: TokenMeasurementSource::Estimated,
            },
            estimated_tokens_after: 1_900,
            occupancy: None,
        },
    });

    let released = projection
        .snapshot_ref()
        .context
        .manual_compaction
        .as_ref()
        .unwrap();
    assert_eq!(
        released
            .request_id
            .as_ref()
            .map(crate::runtime::identity::ManualCompactionRequestId::as_str),
        Some("test-request")
    );
    assert!(released.released);
    assert!(released.error.is_none());
    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    assert!(events[0].event.facts().iter().any(|fact| matches!(
        fact,
        RuntimeClientEvent::ContextCompactionStarted {
            attempt_id: None,
            ..
        }
    )));
    assert!(events[1].event.facts().iter().any(
        |fact| matches!(fact, RuntimeClientEvent::ContextCompacted {
            attempt_id: None,
            context,
        } if !context.compaction_in_progress && context.compaction_count == 1)
    ));
}

/// Turn and usage observations publish the exact values folded into the
/// attempt view, so incremental subscribers stay synchronized with a
/// snapshot without polling.
#[test]
fn turn_and_usage_events_agree_with_the_attempt_view() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    apply_event(&mut projection, RuntimeEvent::TurnStarted);
    apply_event(&mut projection, RuntimeEvent::TurnCompleted);
    apply_event(
        &mut projection,
        RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Agent {
                identity: crate::model::RequestIdentity {
                    attempt_id: attempt(),
                    turn: crate::runtime::identity::TurnId::new("1"),
                    retry_number: 0,
                },
            },
            request_id: RequestId::new("request:9:attempt-1:1:1:0"),
            finish_reason: ModelFinishReason::Stop,
            usage: Some(ModelUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                total_tokens: Some(12),
                details: None,
            }),
            generation: None,
        },
    );
    let events = collect(&mut projection, RuntimeClientCursor::new(1));
    assert_eq!(events.len(), 2);
    assert_eq!(
        *single_fact(&events[0]),
        RuntimeClientEvent::AttemptTurnUpdated {
            attempt_id: attempt(),
            turn: 1,
        }
    );
    assert_eq!(
        *single_fact(&events[1]),
        RuntimeClientEvent::AttemptUsageUpdated {
            attempt_id: attempt(),
            usage: ModelUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                total_tokens: Some(12),
                details: None,
            },
        }
    );
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    let attempt_view = snapshot.attempt.expect("attempt view exists");
    assert_eq!(attempt_view.turn, 1);
    assert_eq!(
        attempt_view
            .last_usage
            .as_ref()
            .and_then(|usage| usage.input_tokens),
        Some(10)
    );
}

#[test]
fn shutdown_folds_into_the_snapshot_and_publishes_the_runtime_fact() {
    let mut projection = projection();
    projection.apply(ConversationObservation::Shutdown);

    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    assert_eq!(events.len(), 1);
    assert_eq!(
        *single_fact(&events[0]),
        RuntimeClientEvent::RuntimeShutdown
    );

    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert!(snapshot.shutting_down);
    assert_eq!(cursor, RuntimeClientCursor::new(1));
}

/// Exactly one terminal settlement exists per attempt, and the
/// terminal client event carries the platform outcome.
#[test]
fn terminal_settlement_is_unique() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptCancelled {
            attempt_id: attempt(),
            reason: CancellationReason::UserRequested,
        },
    );
    let events = collect(&mut projection, RuntimeClientCursor::new(1));
    assert_eq!(
        events
            .iter()
            .filter(|event| event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. })))
            .count(),
        1,
        "exactly one AttemptSettled"
    );
    assert_eq!(
        *single_fact(events.last().expect("terminal event")),
        RuntimeClientEvent::AttemptSettled {
            attempt_id: attempt(),
            outcome: RuntimeClientOutcome::Cancelled {
                reason: CancellationReason::UserRequested,
            },
        }
    );
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert!(matches!(
        snapshot.attempt.expect("attempt view").phase,
        RuntimeClientAttemptPhase::Settled { .. }
    ));
}

/// Provider-specific failure fields never leak: the projected failure
/// drops the provider error code.
#[test]
fn provider_specific_fields_do_not_leak() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptFailed {
            attempt_id: attempt(),
            error: crate::events::types::AttemptFailure::Model {
                error: ModelError {
                    kind: ModelErrorKind::RateLimit,
                    message: "retries exhausted".to_owned(),
                    retry_disposition: crate::model::error::ModelRetryDisposition::Transient,
                    retry_after_ms: Some(5_000),
                    provider_code: Some("rate_limit_exceeded".to_owned()),
                    context_overflow: None,
                    malformed_tool_proposal: None,
                    timeout_phase: None,
                    generation: None,
                },
            },
        },
    );
    let events = collect(&mut projection, RuntimeClientCursor::new(1));
    let settled = events
        .iter()
        .flat_map(|protocol| protocol.event.facts())
        .find_map(|fact| match fact {
            RuntimeClientEvent::AttemptSettled { outcome, .. } => Some(outcome),
            _ => None,
        })
        .expect("terminal settlement");
    let value = serde_json::to_value(settled).expect("serialize outcome");
    assert_eq!(value["type"], "failed");
    let text = serde_json::to_string(settled).expect("serialize");
    assert!(
        !text.contains("provider_code"),
        "provider-specific fields never leak: {text}"
    );
    assert!(text.contains("rate_limit"));
    assert!(text.contains("retries exhausted"));
    assert!(text.contains("5000"));
}

/// Streamed message state folds correctly into the in-flight repair
/// view, and committing the message clears the in-flight state.
#[test]
fn streamed_message_state_folds_correctly() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    apply_publication_open(&mut projection);
    apply_frame(&mut projection, 0, text_frame("hello "));
    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    let in_flight = snapshot
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.in_flight.as_ref())
        .expect("in-flight message");
    assert_eq!(
        in_flight.blocks,
        vec![InFlightBlock::Text {
            block_index: ContentBlockIndex::new(0),
            text: "hello ".to_owned(),
        }]
    );
    // Resume after C delivers exactly the remaining delta, never a
    // duplicate of the accumulated text.
    apply_frame(&mut projection, 1, text_frame("world"));
    let resumed = collect(&mut projection, cursor);
    assert_eq!(resumed.len(), 1, "exactly one delta is published after C");
    assert!(resumed[0].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { kind, inline_delta:Some(delta), .. } if kind == "text" && delta == "world")));
    // The canonical commit clears the in-flight repair state.
    let committed = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("msg-1"),
        content: vec![AssistantContentBlock::Text(TextBlock {
            text: "hello world".to_owned(),
        })],
    });
    projection.apply(ConversationObservation::Committed {
        attempt_id: Some(attempt()),
        block: committed.clone(),
        transcript_cursor: Some(crate::durable::TranscriptCursor::new(1)),
    });
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.messages, vec![committed]);
    assert!(
        snapshot
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.in_flight.as_ref())
            .is_none()
    );
}

/// A durable commit cursor travels with the live observation even when
/// observations arrive in the opposite order. The Runtime Client event
/// cursor records that delivery order only; it is deliberately not the
/// transcript order input.
#[test]
fn live_committed_observations_carry_durable_cursors_across_reordering() {
    let mut projection = projection();
    let message_a = compactable_user("message-a");
    let message_b = compactable_user("message-b");

    projection.apply(ConversationObservation::Committed {
        attempt_id: None,
        block: message_b,
        transcript_cursor: Some(crate::durable::TranscriptCursor::new(11)),
    });
    projection.apply(ConversationObservation::Committed {
        attempt_id: None,
        block: message_a,
        transcript_cursor: Some(crate::durable::TranscriptCursor::new(10)),
    });

    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    assert_eq!(
        events
            .iter()
            .map(|event| event.cursor.get())
            .collect::<Vec<_>>(),
        vec![1, 2],
        "Runtime Client cursors follow observation delivery"
    );
    assert_eq!(
        events
            .iter()
            .flat_map(|event| event.event.facts())
            .filter_map(|fact| match fact {
                RuntimeClientEvent::MessageCommitted {
                    transcript_cursor, ..
                } => transcript_cursor.map(RuntimeClientTranscriptCursor::get),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![11, 10],
        "each live fact carries its own durable transcript cursor"
    );
}

/// Foreground tool state keeps canonical logical identities even when
/// physical completion order is reversed.
#[test]
fn foreground_tool_identity_survives_reversed_completion() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    for (sequence, call) in [
        ToolCallStart {
            id: ToolCallId::new("call_a"),
            tool_id: ToolId::new("tool-alpha"),
            name: "alpha".to_owned(),
        },
        ToolCallStart {
            id: ToolCallId::new("call_b"),
            tool_id: ToolId::new("tool-beta"),
            name: "beta".to_owned(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        apply_frame(
            &mut projection,
            sequence as u64,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(u32::try_from(sequence).unwrap()),
                call,
            },
        );
    }
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call_a"),
            tool_id: ToolId::new("tool-alpha"),
        },
    );
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: ToolCallId::new("call_b"),
            tool_id: ToolId::new("tool-beta"),
        },
    );
    // B settles first physically.
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new("call_b"),
            tool_id: ToolId::new("tool-beta"),
            result: success_result(),
        },
    );
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: ToolCallId::new("call_a"),
            tool_id: ToolId::new("tool-alpha"),
            result: success_result(),
        },
    );
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
    assert_eq!(foreground.len(), 2);
    assert_eq!(foreground[0].block_index, ContentBlockIndex::new(0));
    assert_eq!(foreground[1].block_index, ContentBlockIndex::new(1));
    assert_eq!(foreground[0].message_id, foreground[1].message_id);
    assert_eq!(foreground[0].call_id, Some(ToolCallId::new("call_a")));
    assert_eq!(foreground[1].call_id, Some(ToolCallId::new("call_b")));
    assert!(matches!(
        foreground[0].state,
        ForegroundToolState::Settled { .. }
    ));
    assert!(matches!(
        foreground[1].state,
        ForegroundToolState::Settled { .. }
    ));
}

/// The Runtime Client projection carries the canonical cancellation phase
/// verbatim and preserves it through its wire round trip.
#[test]
fn issue136_foreground_cancellation_phase_round_trips_through_projection() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    let call = ToolCall {
        id: ToolCallId::new("call_cancelled"),
        tool_id: ToolId::new("tool-cancelled"),
        name: "cancelled".to_owned(),
        arguments: serde_json::json!({"value": 1}),
    };
    apply_frame(
        &mut projection,
        0,
        PublicationPayload::ProposedToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
            },
        },
    );
    apply_frame(
        &mut projection,
        1,
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index: ContentBlockIndex::new(0),
            call_id: call.id.clone(),
            suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
        },
    );
    apply_frame(
        &mut projection,
        2,
        PublicationPayload::ProposedToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: call.clone(),
        },
    );
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
    );
    let result = ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::Cancelled {
            reason: CancellationReason::RuntimeShutdown,
            phase: ToolCancellationPhase::DuringExecution,
        },
        content: Vec::new(),
        duration_ms: 3,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    };
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: result.clone(),
        },
    );

    let (snapshot, _) = projection.snapshot().expect("snapshot");
    let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
    let ForegroundToolState::Settled {
        result: projected, ..
    } = &foreground[0].state
    else {
        panic!("the cancelled call must be settled");
    };
    assert_eq!(projected, &result);
    let encoded = serde_json::to_value(&snapshot).expect("snapshot JSON");
    assert_eq!(
        encoded["attempt"]["foreground"][0]["state"]["result"]["status"]["phase"],
        "during_execution"
    );
    let decoded: crate::runtime_client::snapshot::RuntimeClientSnapshot =
        serde_json::from_value(encoded).expect("snapshot round trip");
    assert_eq!(decoded, snapshot);
}

/// The Runtime Client projection carries the canonical `OutcomeUnknown`
/// result verbatim — bounded `detail` included — and preserves it through
/// its wire round trip, never flattening it into a known `failed`.
#[test]
fn issue202_outcome_unknown_flows_through_projection_unchanged() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    let call = ToolCall {
        id: ToolCallId::new("call_outcome_unknown"),
        tool_id: ToolId::new("tool-outcome-unknown"),
        name: "outcome_unknown".to_owned(),
        arguments: serde_json::json!({"value": 1}),
    };
    apply_frame(
        &mut projection,
        0,
        PublicationPayload::ProposedToolCallStarted {
            block_index: ContentBlockIndex::new(0),
            call: ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: call.name.clone(),
            },
        },
    );
    apply_frame(
        &mut projection,
        1,
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index: ContentBlockIndex::new(0),
            call_id: call.id.clone(),
            suffix: serde_json::to_string(&call.arguments).expect("arguments JSON"),
        },
    );
    apply_frame(
        &mut projection,
        2,
        PublicationPayload::ProposedToolCallCompleted {
            block_index: ContentBlockIndex::new(0),
            call: call.clone(),
        },
    );
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
    );
    let result = ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::OutcomeUnknown {
            detail: "executor lost contact after dispatch".to_owned(),
        },
        content: Vec::new(),
        duration_ms: 3,
        exit_code: None,
        artifacts: Vec::new(),
        truncation: None,
        workflow: None,
        managed_output: None,
    };
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: result.clone(),
        },
    );

    let (snapshot, _) = projection.snapshot().expect("snapshot");
    let foreground = &snapshot.attempt.as_ref().expect("attempt view").foreground;
    let ForegroundToolState::Settled {
        result: projected, ..
    } = &foreground[0].state
    else {
        panic!("the outcome-unknown call must be settled");
    };
    assert_eq!(projected, &result);
    let encoded = serde_json::to_value(&snapshot).expect("snapshot JSON");
    let status = &encoded["attempt"]["foreground"][0]["state"]["result"]["status"];
    assert_eq!(status["type"], "outcome_unknown");
    assert_eq!(status["detail"], "executor lost contact after dispatch");
    let decoded: crate::runtime_client::snapshot::RuntimeClientSnapshot =
        serde_json::from_value(encoded).expect("snapshot round trip");
    assert_eq!(decoded, snapshot);
}

/// Canonical result commitment does not create an execution terminal fact.
#[test]
fn issue136_before_start_commit_preserves_execution_distinction() {
    use crate::durable::{ConversationStore, SqliteConversationStore};
    let mut projection = projection();
    let store =
        SqliteConversationStore::in_memory(projection.snapshot.conversation_id.clone()).unwrap();
    let envelope = |event| RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(""),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: Some(attempt()),
        turn_id: None,
        timestamp: chrono::DateTime::UNIX_EPOCH,
        tool_occurrence: None,
        observation: None,
        event,
    };
    let started = store
        .append_event(envelope(RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        }))
        .unwrap();
    projection.apply_native_batch(started.observation.as_ref().unwrap());
    let call = ToolCall {
        id: ToolCallId::new("call_before_start"),
        tool_id: ToolId::new("tool-before-start"),
        name: "before_start".to_owned(),
        arguments: serde_json::json!({"value": 1}),
    };
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("msg-1"),
        content: vec![AssistantContentBlock::ToolCall(call.clone())],
    });
    let (assistant_event, _) = store
        .append_canonical_with_event(
            &assistant,
            envelope(RuntimeEvent::AssistantMessageCommitted {
                message_id: assistant.id().clone(),
            }),
        )
        .unwrap();
    projection.bootstrap_durable_message(&attempt(), &assistant);
    projection.apply_native_batch(assistant_event.observation.as_ref().unwrap());
    let result = cancelled_result(
        CancellationReason::RuntimeShutdown,
        ToolCancellationPhase::BeforeStart,
    );
    let committed = MessageBlock::Tool(ToolMessageBlock {
        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
            assistant.id().clone(),
            ContentBlockIndex::new(0),
        ),
        id: MessageId::new("message-before-start"),
        tool_call_id: call.id.clone(),
        tool_id: call.tool_id.clone(),
        result: result.clone(),
    });
    let (_, before_commit) = projection.snapshot().unwrap();
    let (events, _) = store
        .append_canonical_batch_with_events(
            std::slice::from_ref(&committed),
            &[envelope(RuntimeEvent::ToolMessageCommitted {
                message_id: committed.id().clone(),
                tool_call_id: call.id,
            })],
        )
        .unwrap();
    projection.apply_native_batch(events[0].observation.as_ref().unwrap());
    let events = collect(&mut projection, before_commit);
    let facts: Vec<_> = events
        .iter()
        .flat_map(|event| event.event.facts())
        .collect();
    assert_eq!(facts.iter().filter(|fact| matches!(fact,
        RuntimeClientEvent::ToolResultCommitted { result: projected, .. } if projected == &result
    )).count(), 1);
    assert!(!facts.iter().any(|fact| matches!(
        fact,
        RuntimeClientEvent::ToolExecutionStarted { .. }
            | RuntimeClientEvent::ToolExecutionSettled { .. }
    )));
    let (snapshot, _) = projection.snapshot().unwrap();
    let foreground = &snapshot.attempt.as_ref().unwrap().foreground;
    assert_eq!(foreground.len(), 1);
    assert!(matches!(
        foreground[0].state,
        ForegroundToolState::Assembled { .. }
    ));
    assert_eq!(foreground[0].committed_result.as_ref(), Some(&result));
}

/// A live `DuringExecution` settlement already closes the slot; its later
/// canonical `ToolMessage` commit is history only and emits no duplicate
/// foreground settlement.
#[test]
fn issue136_live_settlement_is_not_duplicated_by_tool_message_commit() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    let call = ToolCall {
        id: ToolCallId::new("call_during_execution"),
        tool_id: ToolId::new("tool-during-execution"),
        name: "during_execution".to_owned(),
        arguments: serde_json::json!({}),
    };
    apply_assembled_call(&mut projection, &call);
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionStarted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
        },
    );
    let result = cancelled_result(
        CancellationReason::ParentCancelled,
        ToolCancellationPhase::DuringExecution,
    );
    apply_event(
        &mut projection,
        RuntimeEvent::ToolExecutionCompleted {
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: result.clone(),
        },
    );
    let (_, before_commit) = projection.snapshot().expect("snapshot before commit");
    projection.apply(ConversationObservation::Committed {
        attempt_id: Some(attempt()),
        block: MessageBlock::Tool(ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new("msg-1"),
                crate::message::types::ContentBlockIndex::new(0),
            ),
            id: MessageId::new("message-during-execution"),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: result.clone(),
        }),
        transcript_cursor: Some(crate::durable::TranscriptCursor::new(1)),
    });

    let commit_events = collect(&mut projection, before_commit);
    assert_eq!(commit_events.len(), 1);
    assert!(
        commit_events[0]
            .event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::MessageCommitted { .. }))
    );
    let (snapshot, _) = projection.snapshot().expect("snapshot after commit");
    assert!(matches!(
        &snapshot.attempt.as_ref().expect("attempt view").foreground[0].state,
        ForegroundToolState::Settled { result: projected, .. } if projected == &result
    ));
}

/// A resume after a serviceable cursor replays exactly the retained
/// gap, in order, without duplicates or gaps.
#[test]
fn resume_after_a_serviceable_cursor_has_no_gap() {
    let mut projection = projection();
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    apply_publication_open(&mut projection);
    let (_, opened_cursor) = projection.snapshot().unwrap();
    assert_eq!(
        opened_cursor,
        RuntimeClientCursor::new(2),
        "Attempt start and genuine stream opening precede suffixes"
    );
    for index in 0..10u64 {
        apply_frame(&mut projection, index, text_frame(&index.to_string()));
    }
    let (_, cursor) = projection.snapshot().expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(12));
    let events = collect(&mut projection, RuntimeClientCursor::new(3));
    let cursors: Vec<u64> = events.iter().map(|event| event.cursor.get()).collect();
    assert_eq!(cursors, vec![4, 5, 6, 7, 8, 9, 10, 11, 12]);
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].cursor < pair[1].cursor)
    );
}

/// An expired cursor (evicted from the bounded ring) and a cursor
/// ahead of the stream both fail explicitly with `resync_required`;
/// a fresh snapshot still repairs all state.
#[test]
fn unserviceable_cursors_fail_with_resync_required() {
    let mut projection = RuntimeClientProjection::new(
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        Vec::new(),
        crate::observation::capability_source::CapabilityView {
            configured_tools: Vec::new(),
            revision: crate::runtime::identity::CapabilityRevision::new(1),
            tools: Vec::new(),
            available_tools: Vec::new(),
            skills: Vec::new(),
            sources: Vec::new(),
        },
        Some(model_view()),
        4,
    );
    apply_publication_open(&mut projection);
    for index in 0..20u64 {
        apply_frame(&mut projection, index, text_frame(&index.to_string()));
    }
    let error = projection
        .subscribe(RuntimeClientCursor::new(1))
        .expect_err("the cursor was evicted from the bounded ring");
    match error {
        crate::runtime_client::types::RuntimeClientError::ResyncRequired {
            after_cursor,
            earliest_serviceable,
        } => {
            assert_eq!(after_cursor, RuntimeClientCursor::new(1));
            assert_eq!(earliest_serviceable, RuntimeClientCursor::new(17));
        }
        other => panic!("expected resync_required, got {other:?}"),
    }
    let error = projection
        .subscribe(RuntimeClientCursor::new(100))
        .expect_err("a cursor ahead of the stream is unserviceable");
    assert!(matches!(
        error,
        crate::runtime_client::types::RuntimeClientError::ResyncRequired { .. }
    ));
    // A fresh snapshot repairs every externally visible fact.
    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(21));
    let in_flight = snapshot
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.in_flight.as_ref())
        .expect("in-flight view repaired");
    assert!(matches!(
        &in_flight.blocks[0],
        InFlightBlock::Text { text, .. } if text == "012345678910111213141516171819"
    ));
}

/// A stalled subscriber introduces no second backlog: the bounded
/// replay ring is the only retained storage, its size never exceeds the
/// retention limit no matter how far behind the consumer is, and the
/// consumer is told explicitly that it fell behind rather than being
/// handed a silently non-contiguous stream.
#[test]
fn a_stalled_subscriber_is_bounded_and_never_silently_skips() {
    let limit = 4;
    let mut projection = RuntimeClientProjection::new(
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        Vec::new(),
        crate::observation::capability_source::CapabilityView {
            configured_tools: Vec::new(),
            revision: crate::runtime::identity::CapabilityRevision::new(1),
            tools: Vec::new(),
            available_tools: Vec::new(),
            skills: Vec::new(),
            sources: Vec::new(),
        },
        Some(model_view()),
        limit,
    );
    apply_publication_open(&mut projection);
    let (stalled, _notify) = projection
        .subscribe(RuntimeClientCursor::new(1))
        .expect("the current cursor is serviceable");

    // The subscriber never polls while 200 events are published.
    for index in 0..200u64 {
        apply_frame(&mut projection, index, text_frame(&index.to_string()));
        assert!(
            projection.replay_len() <= limit,
            "the one retained backlog stays within its bound at every publication"
        );
    }
    assert_eq!(
        projection.replay_len(),
        limit,
        "retention is the only storage that grew, and only to its bound"
    );

    // Publication cost per subscriber is one wakeup, so the stalled
    // subscriber holds nothing: it is still sitting at its original
    // cursor.
    let poll = projection.poll_subscriber(stalled);
    let SubscriberPoll::Lagged {
        after_cursor,
        earliest_serviceable,
    } = poll
    else {
        panic!("a subscriber past retention must lag explicitly, got {poll:?}");
    };
    assert_eq!(after_cursor, RuntimeClientCursor::new(1));
    assert_eq!(earliest_serviceable, RuntimeClientCursor::new(197));
    assert_eq!(
        projection.poll_subscriber(stalled),
        poll,
        "the lag verdict is stable and never partially advances the cursor"
    );

    // A consumer that keeps up observes strictly contiguous cursors,
    // one event per poll, and then parks.
    let (live, _notify) = projection
        .subscribe(projection.cursor())
        .expect("the current cursor is serviceable");
    let base = projection.cursor().get();
    for step in 1..=3u64 {
        apply_frame(&mut projection, 199 + step, text_frame("x"));
        let poll = projection.poll_subscriber(live);
        let SubscriberPoll::Event(event) = poll else {
            panic!("the live subscriber must receive, got {poll:?}");
        };
        assert_eq!(event.cursor.get(), base + step);
    }
    assert_eq!(projection.poll_subscriber(live), SubscriberPoll::Pending);

    // Removal is observable, never an indefinite park.
    projection.remove_subscriber(live);
    assert_eq!(projection.poll_subscriber(live), SubscriberPoll::Closed);
}

/// Cursor overflow fails explicitly and never wraps: publishing stops,
/// subscriptions fail with a typed error, and reads no longer hand back
/// a read model that silently stopped folding transitions.
#[test]
fn cursor_overflow_fails_explicitly() {
    let mut projection = projection();
    projection.force_cursor_for_test(u64::MAX - 1);
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    assert_eq!(projection.cursor(), RuntimeClientCursor::new(u64::MAX));
    apply_event(&mut projection, RuntimeEvent::TurnStarted);
    apply_frame(&mut projection, 0, text_frame("x"));
    assert_eq!(
        projection.cursor(),
        RuntimeClientCursor::new(u64::MAX),
        "the cursor never wraps"
    );
    assert!(matches!(
        projection.subscribe(RuntimeClientCursor::new(0)),
        Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
    ));
    // Reads fail explicitly too: after exhaustion the projection can no
    // longer fold authoritative transitions, and returning the stale
    // read model would hide exactly that.
    assert!(matches!(
        projection.snapshot(),
        Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
    ));
    assert!(matches!(
        projection.snapshot_ref_checked(),
        Err(crate::runtime_client::types::RuntimeClientError::ProjectionExhausted)
    ));
}

/// Mailbox observations fold into the inbound diagnostics view:
/// enqueues appear as pending in inbound-sequence order and a finite
/// drain clears them and records the watermark.
#[test]
fn inbound_diagnostics_fold_from_mailbox_observations() {
    let mailbox = ConversationInboundMailbox::new(ConversationId::new(
        "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
    ));
    let item = |id: &str| UserMessageBlock {
        id: MessageId::new(id),
        content: vec![UserContentBlock::Text(TextBlock {
            text: id.to_owned(),
        })],
        source: UserSource::Human,
        kind: InboundKind::Message,
        timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("parse")
                .with_timezone(&chrono::Utc),
        ),
    };
    let first = mailbox.enqueue(item("m1")).expect("enqueue");
    let second = mailbox.enqueue(item("m2")).expect("enqueue");

    let mut projection = projection();
    let drained = mailbox
        .select_pending_batch()
        .expect("select")
        .expect("batch");
    let _ = (first, second);
    // The authoritative items fold through the enqueue observations.
    for entry in drained.items() {
        projection.apply(ConversationObservation::InboundEnqueued(entry.clone()));
    }
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.inbound.pending.len(), 2);
    assert_eq!(
        snapshot.inbound.pending[0].sequence.get(),
        1,
        "pending items follow inbound sequence order"
    );
    assert_eq!(snapshot.inbound.pending[1].sequence.get(), 2);

    // The finite drain of the same batch clears the pending view and
    // records the watermark.
    projection.apply(ConversationObservation::InboundDrained(drained));
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert!(snapshot.inbound.pending.is_empty());
    let last_drain = snapshot.inbound.last_drain.expect("drain recorded");
    assert_eq!(last_drain.watermark.get(), 2);
    assert_eq!(last_drain.count, 2);
}

/// Native interactions are live Runtime Client projection facts: a
/// pending event adds exactly one deterministic snapshot entry, and its
/// terminal event removes that entry without touching canonical messages.
#[test]
#[allow(clippy::too_many_lines)]
fn interaction_pending_and_settled_fold_into_snapshot_and_events() {
    let mut projection = projection();
    let request = InteractionRequest {
        id: InteractionId::new("attempt-1-interaction-1"),
        conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        execution: rustx::runtime::interaction::InteractionExecution::Attempt {
            attempt_id: attempt(),
            turn: 2,
        },
        kind: InteractionKind::Approval {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("call-1"),
            },
            tool_id: ToolId::new("tool-alpha"),
            tool_name: "alpha".into(),
            origin: crate::tools::types::ToolOrigin::Builtin,
            mode: crate::tools::types::ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"path": "original"}),
            reason: "native policy".to_owned(),
        },
    };
    let requested_audit = interaction_audit(
        &request,
        "event-requested",
        RuntimeEvent::InteractionRequested {
            interaction_id: request.id.clone(),
            subject: InteractionSubject::Approval {
                invocation_id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new("call-1"),
                },
                tool_id: ToolId::new("tool-alpha"),
                tool_name: "alpha".into(),
                arguments_digest: "0".repeat(64),
                reason: "native policy".to_owned(),
            },
        },
    );
    projection.apply(ConversationObservation::InteractionPending {
        interaction: crate::observation::interaction::interaction_request_header(
            &request,
            InteractionSource::Primary,
        ),
        audit: Some((requested_audit, crate::durable::TranscriptCursor::new(1))),
    });
    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert_eq!(
        snapshot.pending_interactions,
        vec![crate::observation::interaction::interaction_request_header(
            &request,
            InteractionSource::Primary
        )]
    );
    assert_eq!(cursor, RuntimeClientCursor::new(1));
    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    assert!(matches!(
        events.first().and_then(|event| event.event.facts().iter().find(|fact| matches!(fact, RuntimeClientEvent::InteractionPending { .. }))),
        Some(RuntimeClientEvent::InteractionPending { interaction })
            if interaction.request_id == request.id && interaction.execution == request.execution && !interaction.body_complete
    ));

    let outcome = InteractionOutcome::Responded {
        response: InteractionResponse::Approval {
            decision: ApprovalDecision::Allow,
        },
    };
    let settled_audit = interaction_audit(
        &request,
        "event-settled",
        RuntimeEvent::InteractionSettled {
            interaction_id: request.id.clone(),
            settlement: InteractionSettlement::Approved,
        },
    );
    projection.apply(ConversationObservation::InteractionSettled {
        interaction: InteractionRef::new(request.conversation_id.clone(), request.id.clone()),
        outcome: outcome.clone(),
        audit: Some((settled_audit, crate::durable::TranscriptCursor::new(2))),
    });
    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert!(snapshot.pending_interactions.is_empty());
    assert_eq!(cursor, RuntimeClientCursor::new(2));
    let events = collect(&mut projection, RuntimeClientCursor::new(1));
    assert!(matches!(
        events.first().and_then(|event| event.event.facts().iter().find(|fact| matches!(fact, RuntimeClientEvent::InteractionSettled { .. }))),
        Some(RuntimeClientEvent::InteractionSettled {
            interaction,
            outcome: event_outcome,
        }) if interaction
            == &InteractionRef::new(request.conversation_id.clone(), request.id.clone())
            && event_outcome == &outcome
    ));

    // The projection never turns a live prompt into a historical audit;
    // only the durable requested/settled audit observations do that.
    assert!(
        projection
            .snapshot()
            .expect("snapshot")
            .0
            .messages
            .is_empty()
    );
}

/// The root projection is a set of independently addressed routed
/// interactions. Primary and child prompts can coexist, and removing or
/// settling one pair never selects or changes another pair.
#[test]
fn routed_interactions_keep_multiple_sources_and_removals_independent() {
    let mut projection = projection();
    let approval = |conversation: &str, id: &str| InteractionRequest {
        id: InteractionId::new(id),
        conversation_id: ConversationId::new(conversation),
        execution: rustx::runtime::interaction::InteractionExecution::Attempt {
            attempt_id: attempt(),
            turn: 1,
        },
        kind: InteractionKind::Approval {
            invocation_id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new(format!("{id}-call")),
            },
            tool_id: ToolId::new("tool-alpha"),
            tool_name: "alpha".into(),
            origin: crate::tools::types::ToolOrigin::Builtin,
            mode: crate::tools::types::ToolInvocationMode::Foreground,
            arguments: serde_json::json!({"id": id}),
            reason: "independent prompt".to_owned(),
        },
    };
    let primary = RoutedInteraction::primary(approval(
        "conv_4cd896f4-a321-7e22-9d65-c0862e12ef89",
        "primary-a",
    ));
    let child_a_request = approval("conv_8e708397-e71d-7840-8b5a-5074c7b2d0ed", "child-a");
    let child_a = RoutedInteraction::subagent(
        crate::runtime::identity::SubagentId::new(
            "conv_4cd896f4-a321-7e22-9d65-c0862e12ef89-subagent-1",
        ),
        child_a_request.conversation_id.clone(),
        crate::runtime::subagent::catalog::SubagentName::parse("explore").expect("agent name"),
        child_a_request,
    );
    let other_child_request = approval("conv_143069cc-2a88-7514-b43a-0dfdaa9f1170", "child-b");
    let child_b = RoutedInteraction::subagent(
        crate::runtime::identity::SubagentId::new(
            "conv_4cd896f4-a321-7e22-9d65-c0862e12ef89-subagent-2",
        ),
        other_child_request.conversation_id.clone(),
        crate::runtime::subagent::catalog::SubagentName::parse("explore").expect("agent name"),
        other_child_request,
    );

    for interaction in [primary.clone(), child_a.clone(), child_b.clone()] {
        projection.apply(ConversationObservation::InteractionPending {
            interaction: crate::observation::interaction::interaction_header(&interaction),
            audit: None,
        });
    }
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.pending_interactions.len(), 3);
    assert_eq!(
        snapshot
            .pending_interactions
            .iter()
            .map(|interaction| interaction.interaction.clone())
            .collect::<Vec<_>>(),
        vec![
            primary.interaction.clone(),
            child_a.interaction.clone(),
            child_b.interaction.clone(),
        ]
    );

    projection.apply(ConversationObservation::InteractionRemoved {
        interaction: child_a.interaction.clone(),
    });
    let (snapshot, _) = projection.snapshot().expect("snapshot after child removal");
    assert_eq!(
        snapshot
            .pending_interactions
            .iter()
            .map(|interaction| interaction.interaction.clone())
            .collect::<Vec<_>>(),
        vec![primary.interaction.clone(), child_b.interaction.clone()]
    );

    projection.apply(ConversationObservation::InteractionSettled {
        interaction: child_b.interaction.clone(),
        outcome: InteractionOutcome::Responded {
            response: InteractionResponse::Approval {
                decision: ApprovalDecision::Allow,
            },
        },
        audit: None,
    });
    let (snapshot, _) = projection.snapshot().expect("snapshot after settlement");
    assert_eq!(
        snapshot.pending_interactions,
        vec![crate::observation::interaction::interaction_header(
            &primary
        )]
    );
    assert!(snapshot.messages.is_empty());
}

/// The snapshot and its cursor linearize: the returned cursor is the
/// cursor of the returned snapshot.
#[test]
fn snapshot_and_cursor_are_linearized() {
    let mut projection = projection();
    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert!(snapshot.messages.is_empty());
    apply_event(
        &mut projection,
        RuntimeEvent::AttemptStarted {
            attempt_id: attempt(),
        },
    );
    let (snapshot, cursor) = projection.snapshot().expect("snapshot");
    assert_eq!(cursor, RuntimeClientCursor::new(1));
    assert!(snapshot.attempt.is_some());
}

/// Background registry observations fold into the background section
/// preserving newest-first allocation order and terminal retention.
#[test]
fn background_observations_fold_in_allocation_order() {
    use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
    let mut projection = projection();
    projection.apply(ConversationObservation::Background(
        BackgroundExecutionSnapshot {
            bash: None,
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
            ),
            tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
            tool_name: "bg".into(),
            state: BackgroundLifecycle::Running,
            progress: None,
            result: None,
        },
    ));
    projection.apply(ConversationObservation::Background(
        BackgroundExecutionSnapshot {
            bash: None,
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
            ),
            tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
            tool_name: "bg".into(),
            state: BackgroundLifecycle::Succeeded,
            progress: None,
            result: Some(std::sync::Arc::new(success_result())),
        },
    ));
    projection.apply(ConversationObservation::Background(
        BackgroundExecutionSnapshot {
            bash: None,
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_20eb7fc0-b69d-7476-8553-c156fdc879c3",
            ),
            tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
            tool_name: "bg".into(),
            state: BackgroundLifecycle::Starting,
            progress: None,
            result: None,
        },
    ));
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.jobs.len(), 2);
    assert_eq!(
        snapshot.jobs[1].job_id.as_str(),
        "exec_215a03ee-2332-70b6-8e2d-634da8066f98"
    );
    assert_eq!(snapshot.jobs[1].state, BackgroundLifecycle::Succeeded);
    assert!(snapshot.jobs[1].result.is_some());
    assert_eq!(
        snapshot.jobs[0].job_id.as_str(),
        "exec_20eb7fc0-b69d-7476-8553-c156fdc879c3"
    );
}

/// Issue #202: the projection carries the honest background terminal
/// lifecycle vocabulary verbatim. `OutcomeUnknown` and `TimedOut` are
/// first-class terminal states and are never collapsed into `Failed`,
/// in neither the folded view nor the published event, on or off the
/// wire.
#[test]
fn issue202_background_terminal_states_project_verbatim() {
    use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
    let mut projection = projection();
    let outcome_unknown_result = ToolExecutionResult {
        deliveries: Vec::new(),
        status: ToolExecutionStatus::OutcomeUnknown {
            detail: "cancellation raced the executor after dispatch".to_owned(),
        },
        ..success_result()
    };
    projection.apply(ConversationObservation::Background(
        BackgroundExecutionSnapshot {
            bash: None,
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_60002c8f-aeff-7c09-8709-610f0ed8d415",
            ),
            tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
            tool_name: "bg".into(),
            state: BackgroundLifecycle::OutcomeUnknown,
            progress: None,
            result: Some(std::sync::Arc::new(outcome_unknown_result)),
        },
    ));
    projection.apply(ConversationObservation::Background(
        BackgroundExecutionSnapshot {
            bash: None,
            execution_id: crate::runtime::identity::ToolExecutionId::new(
                "exec_d013aabc-70aa-72fc-8b80-81767d9a64a1",
            ),
            tool_id: std::sync::Arc::new(ToolId::new("tool-bg")),
            tool_name: "bg".into(),
            state: BackgroundLifecycle::TimedOut,
            progress: None,
            result: Some(std::sync::Arc::new(ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::TimedOut,
                ..success_result()
            })),
        },
    ));

    // The folded view keeps the typed terminal states verbatim.
    let (snapshot, _) = projection.snapshot().expect("snapshot");
    assert_eq!(snapshot.jobs.len(), 2);
    assert_eq!(snapshot.jobs[1].state, BackgroundLifecycle::OutcomeUnknown);
    assert_eq!(snapshot.jobs[0].state, BackgroundLifecycle::TimedOut);

    // The published events carry the same verbatim states.
    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    let [unknown_event, timed_out_event] = events.as_slice() else {
        panic!("two JobUpdated events: {events:?}");
    };
    let RuntimeClientEvent::JobUpdated {
        job: unknown_view, ..
    } = single_fact(unknown_event)
    else {
        panic!("a JobUpdated event: {:?}", unknown_event.event);
    };
    let RuntimeClientEvent::JobUpdated {
        job: timed_out_view,
        ..
    } = single_fact(timed_out_event)
    else {
        panic!("a JobUpdated event: {:?}", timed_out_event.event);
    };
    assert_eq!(unknown_view.state, BackgroundLifecycle::OutcomeUnknown);
    assert_eq!(timed_out_view.state, BackgroundLifecycle::TimedOut);

    // On the wire the states serialize as their typed snake_case names,
    // never "failed".
    let unknown_json = serde_json::to_value(unknown_view).expect("execution JSON");
    assert_eq!(unknown_json["state"], "outcome_unknown");
    let timed_out_json = serde_json::to_value(timed_out_view).expect("execution JSON");
    assert_eq!(timed_out_json["state"], "timed_out");
}

/// Issue #178: a subagent registry snapshot folds into the whole-view
/// `AgentUpdated` event carrying the live activity projection and
/// the redacted execution profile, and the snapshot repair path serves
/// the same enriched view.
#[test]
fn subagent_observations_fold_the_activity_projection_into_the_view() {
    use crate::runtime::subagent::{
        SubagentActivity, SubagentActivityCounters, SubagentExecutionProfile, SubagentObservation,
    };

    let mut projection = projection();
    let observation = SubagentObservation {
        revision: 2,
        activity: SubagentActivity::ToolGroup {
            active: 1,
            representative: Some(
                crate::runtime::subagent::activity::SubagentToolRepresentative {
                    tool_call_id: ToolCallId::new("call-1"),
                    tool_id: ToolId::new("tool-bash"),
                    progress: None,
                },
            ),
        },
        counters: SubagentActivityCounters {
            tool_executions: 1,
            ..SubagentActivityCounters::default()
        },
        ..SubagentObservation::default()
    };
    projection.apply(owner_observation(
        agent_activation_snapshot(observation.clone()),
        crate::runtime::subagent::AgentState::Active,
    ));
    // The disposable lane enriches the current owner with newer activity.
    let newer = SubagentObservation {
        revision: 3,
        ..observation.clone()
    };
    projection.apply(ConversationObservation::SubagentActivity(
        agent_activation_snapshot(newer.clone()),
    ));

    // Both events carry the enriched whole view; the latest wins.
    let events = collect(&mut projection, RuntimeClientCursor::new(0));
    let [_, event] = events.as_slice() else {
        panic!("two AgentUpdated events: {events:?}");
    };
    let RuntimeClientEvent::AgentUpdated {
        agent: subagent, ..
    } = single_fact(event)
    else {
        panic!("a AgentUpdated event: {:?}", event.event);
    };
    assert_eq!(subagent.observation, newer);
    assert_eq!(
        subagent.execution_profile,
        Some(SubagentExecutionProfile {
            model: "local/model".to_owned(),
            profile: None,
            reasoning_enabled: false,
        })
    );
    assert_eq!(
        subagent.started_at,
        chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc)
    );

    // The snapshot repair path re-reads the same enriched view.
    let (repaired, _) = projection.snapshot().expect("snapshot");
    assert_eq!(repaired.agents.len(), 1);
    assert_eq!(repaired.agents[0].observation, newer);
    assert_eq!(
        repaired.agents[0].execution_profile,
        subagent.execution_profile
    );
}
fn committed_settlement() -> crate::runtime::subagent::SubagentSettlement {
    crate::runtime::subagent::SubagentSettlement {
        publication: crate::runtime::subagent::SubagentPublication::Committed,
        physical: crate::runtime::subagent::SubagentPhysicalSettlement::Proven,
    }
}

fn owner_snapshot(
    activation: &crate::runtime::subagent::SubagentSnapshot,
    state: crate::runtime::subagent::AgentState,
) -> crate::runtime::subagent::AgentSnapshot {
    crate::runtime::subagent::AgentSnapshot {
        title: "Explore task".into(),
        agent_id: activation.child_agent_id.clone(),
        conversation_id: activation.child_conversation_id.clone(),
        parent_agent_id: activation.parent_agent_id.clone(),
        agent: activation.agent.clone(),
        state,
        current_activation: (state != crate::runtime::subagent::AgentState::Inactive)
            .then(|| activation.subagent_id.clone()),
        latest_activation: activation.subagent_id.clone(),
        observation: activation.observation.clone(),
    }
}

fn owner_observation(
    snapshot: crate::runtime::subagent::SubagentSnapshot,
    state: crate::runtime::subagent::AgentState,
) -> ConversationObservation {
    ConversationObservation::SubagentLifecycle {
        agent: Some(Box::new(owner_snapshot(&snapshot, state))),
        snapshot,
    }
}

/// Folding exactly one owner observation is the publication cut. No client
/// lifecycle inference or second owner observation can repair this result.
#[test]
fn owner_transition_projects_starting_and_terminal_unproven_at_one_cut() {
    use crate::runtime::subagent::{AgentState, SubagentPublication, SubagentState};
    for (activation_state, owner_state) in [
        (SubagentState::Stopping, AgentState::Admitting),
        (SubagentState::Interrupted, AgentState::Unavailable),
    ] {
        let mut projection = projection();
        let mut activation =
            agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
        activation.state = activation_state;
        if activation_state.is_terminal() {
            activation.settlement.publication = SubagentPublication::Committed;
        }
        let owner = owner_snapshot(&activation, owner_state);
        let fresh = super::agent_view(&owner, &activation);
        projection.apply(ConversationObservation::SubagentLifecycle {
            agent: Some(Box::new(owner)),
            snapshot: activation.clone(),
        });
        let (snapshot, _) = projection.snapshot().unwrap();
        assert_eq!(snapshot.agents, vec![fresh.clone()]);
        let events = collect(&mut projection, RuntimeClientCursor::new(0));
        assert_eq!(events.len(), 1, "one complete owner transition");
        assert!(
            events[0].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AgentUpdated { agent, .. } if **agent == fresh))
        );
        // Disposable activation data can never replace owner lifecycle.
        activation.observation.revision += 1;
        projection.apply(ConversationObservation::SubagentActivity(activation));
        assert_eq!(
            projection.snapshot().unwrap().0.agents[0].state,
            owner_state
        );
    }
}

#[test]
fn subagent_activity_updates_only_newer_live_observations() {
    use crate::runtime::subagent::{
        AgentState, SubagentActivity, SubagentObservation, SubagentState,
    };
    let mut projection = projection();
    let mut owner = agent_activation_snapshot(SubagentObservation::default());
    owner.state = SubagentState::Stopping;
    owner.observation.revision = 2;
    projection.apply(owner_observation(owner.clone(), AgentState::Active));
    let expected = projection.snapshot().unwrap().0.agents[0].clone();
    let mut activity = owner.clone();
    activity.state = SubagentState::Running;
    activity.detail = Some("stale owner diagnostic".into());
    activity.observation.revision = 3;
    activity.observation.activity = SubagentActivity::Compacting;
    projection.apply(ConversationObservation::SubagentActivity(activity.clone()));
    let mut enriched = expected;
    enriched.observation = activity.observation.clone();
    assert_eq!(projection.snapshot().unwrap().0.agents, vec![enriched]);
    let current = projection.snapshot().unwrap();
    projection.apply(ConversationObservation::SubagentActivity(owner.clone()));
    projection.apply(ConversationObservation::SubagentActivity(activity.clone()));
    assert_eq!(
        projection.snapshot().unwrap(),
        current,
        "old and repeated revisions cannot regress activity"
    );
    owner.state = SubagentState::Succeeded;
    owner.settlement = committed_settlement();
    owner.observation = activity.observation.clone();
    owner.observation.settle_neutral();
    projection.apply(owner_observation(owner, AgentState::Inactive));
    let terminal = projection.snapshot().unwrap();
    activity.observation.revision = 100;
    projection.apply(ConversationObservation::SubagentActivity(activity));
    assert_eq!(
        projection.snapshot().unwrap(),
        terminal,
        "terminal activity stays neutral even if a late envelope claims a newer revision"
    );
}

#[test]
fn agents_and_jobs_share_incremental_cursor_and_captured_snapshot_cut() {
    use crate::runtime::subagent::{AgentState, SubagentObservation};
    use crate::tools::background::{BackgroundExecutionSnapshot, BackgroundLifecycle};
    let mut projection = projection();
    let fence = projection.read_domain_fence();
    let mut activation = agent_activation_snapshot(SubagentObservation::default());
    let job = BackgroundExecutionSnapshot {
        bash: None,
        execution_id: crate::runtime::identity::ToolExecutionId::new(
            "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
        ),
        tool_id: std::sync::Arc::new(ToolId::new("bash")),
        tool_name: "bash".into(),
        state: BackgroundLifecycle::Running,
        progress: None,
        result: None,
    };
    projection.apply(owner_observation(activation.clone(), AgentState::Active));
    projection.apply(ConversationObservation::Background(job.clone()));
    let candidate = projection.snapshot_candidate().unwrap();
    assert_eq!(candidate.cursor, RuntimeClientCursor::new(2));
    let captured_activation = candidate.snapshot.agents[0].activation_id.clone();
    activation.subagent_id =
        crate::runtime::identity::SubagentId::new("conv_57d68983-5497-771e-8aaa-5f1356061698");
    projection.apply(owner_observation(activation.clone(), AgentState::Active));
    projection.apply(ConversationObservation::Background(
        BackgroundExecutionSnapshot {
            bash: None,
            state: BackgroundLifecycle::Succeeded,
            result: Some(std::sync::Arc::new(success_result())),
            ..job
        },
    ));
    assert_eq!(
        projection.read_domain_fence(),
        fence,
        "domain observations never request a durable Session reread"
    );
    assert_eq!(
        candidate.snapshot.agents[0].activation_id,
        captured_activation
    );
    assert_eq!(
        candidate.snapshot.jobs[0].state,
        BackgroundLifecycle::Running
    );
    let events = collect(&mut projection, candidate.cursor);
    assert_eq!(
        events.len(),
        2,
        "no unrelated Trace/read-domain invalidation"
    );
    assert_eq!(events[0].cursor, RuntimeClientCursor::new(3));
    assert_eq!(events[1].cursor, RuntimeClientCursor::new(4));
    assert!(
        events[0].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AgentUpdated { agent, .. } if agent.activation_id == activation.subagent_id))
    );
    assert!(
        events[1].event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::JobUpdated { job, .. } if job.state == BackgroundLifecycle::Succeeded))
    );
    assert_eq!(
        candidate.snapshot.agents[0].activation_id, captured_activation,
        "replay cannot mutate an earlier captured activation"
    );
}

fn agent_activation_snapshot(
    observation: crate::runtime::subagent::SubagentObservation,
) -> crate::runtime::subagent::SubagentSnapshot {
    use crate::runtime::subagent::{SubagentExecutionProfile, SubagentSnapshot, SubagentState};
    SubagentSnapshot {
        ownership: crate::events::types::SubagentOwnershipKind::Normal,
        parent_agent_id: AgentId::new("agent-parent"),
        subagent_id: crate::runtime::identity::SubagentId::new(
            "conv_57d68983-5497-771e-8aaa-5f1356061697",
        ),
        child_agent_id: AgentId::new("agent-child"),
        child_conversation_id: ConversationId::new("conv_57d68983-5497-771e-8aaa-5f1356061697"),
        origin: crate::runtime::subagent::AgentActivationOrigin::CreationTool {
            tool_call_id: ToolCallId::new("call-1"),
        },
        agent: "explore".to_owned(),
        definition_digest: "sha256:d1".to_owned(),
        profile_digest: "sha256:p1".to_owned(),
        workspace: std::path::PathBuf::from("/shared-workspace"),
        state: SubagentState::Running,
        cancel_reason: None,
        detail: None,
        observation,
        profile: Some(std::sync::Arc::new(SubagentExecutionProfile {
            model: "local/model".to_owned(),
            profile: None,
            reasoning_enabled: false,
        })),
        settlement: crate::runtime::subagent::SubagentSettlement::default(),
        started_at: chrono::DateTime::parse_from_rfc3339("2026-09-02T10:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc),
    }
}
#[test]
fn durable_agent_projection_preserves_identity_across_resume_and_rejects_old_activity() {
    use crate::runtime::subagent::{AgentState, SubagentState};
    let mut projection = projection();
    let first = agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
    projection.apply(owner_observation(first.clone(), AgentState::Active));
    let mut completed = first.clone();
    completed.state = SubagentState::Succeeded;
    completed.settlement = committed_settlement();
    projection.apply(owner_observation(completed.clone(), AgentState::Inactive));
    let (inactive, _) = projection.snapshot().unwrap();
    assert_eq!(inactive.agents.len(), 1);
    assert_eq!(inactive.agents[0].state, AgentState::Inactive);
    assert_eq!(inactive.agents[0].current_activation, None);
    projection.apply(ConversationObservation::SubagentLifecycle {
        snapshot: completed,
        agent: Some(Box::new(crate::runtime::subagent::AgentSnapshot {
            title: "Explore task".into(),
            agent_id: first.child_agent_id.clone(),
            conversation_id: first.child_conversation_id.clone(),
            parent_agent_id: first.parent_agent_id.clone(),
            agent: first.agent.clone(),
            state: AgentState::Admitting,
            current_activation: Some(crate::runtime::identity::SubagentId::new("activation-next")),
            latest_activation: first.subagent_id.clone(),
            observation: first.observation.clone(),
        })),
    });
    let (reserving, _) = projection.snapshot().unwrap();
    assert_eq!(reserving.agents[0].state, AgentState::Admitting);
    assert_eq!(
        reserving.agents[0].current_activation,
        Some(crate::runtime::identity::SubagentId::new("activation-next"))
    );
    let mut resumed = first.clone();
    resumed.subagent_id = crate::runtime::identity::SubagentId::new("activation-next");
    projection.apply(owner_observation(resumed.clone(), AgentState::Active));
    let (_, resume_cursor) = projection.snapshot().unwrap();
    projection.apply(ConversationObservation::SubagentActivity(first.clone()));
    let (reconnected, cursor) = projection.snapshot().unwrap();
    assert_eq!(
        cursor, resume_cursor,
        "stale old activity publishes no update"
    );
    assert_eq!(reconnected.agents.len(), 1);
    let agent = &reconnected.agents[0];
    assert_eq!(agent.agent_id, first.child_agent_id);
    assert_eq!(agent.child_conversation_id, first.child_conversation_id);
    assert_eq!(agent.state, AgentState::Active);
    assert_eq!(agent.current_activation, Some(resumed.subagent_id.clone()));
    assert_eq!(agent.activation_id, resumed.subagent_id);
    let replay = collect(&mut projection, RuntimeClientCursor::new(0));
    let states: Vec<_> = replay
        .iter()
        .flat_map(|event| event.event.facts())
        .filter_map(|fact| match fact {
            RuntimeClientEvent::AgentUpdated { agent, .. } => {
                Some((agent.agent_id.clone(), agent.state))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec![
            (first.child_agent_id.clone(), AgentState::Active),
            (first.child_agent_id.clone(), AgentState::Inactive),
            (first.child_agent_id.clone(), AgentState::Admitting),
            (first.child_agent_id, AgentState::Active)
        ]
    );
}

#[test]
fn authoritative_agent_bootstrap_keeps_latest_activation_then_accepts_live_resume() {
    use crate::runtime::subagent::{AgentSnapshot, AgentState, SubagentState};
    let mut projection = projection();
    let mut latest =
        agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
    latest.subagent_id = crate::runtime::identity::SubagentId::new("activation-3");
    latest.state = SubagentState::Succeeded;
    latest.settlement = committed_settlement();
    let owner = AgentSnapshot {
        title: "Explore task".into(),
        agent_id: latest.child_agent_id.clone(),
        conversation_id: latest.child_conversation_id.clone(),
        parent_agent_id: latest.parent_agent_id.clone(),
        agent: latest.agent.clone(),
        state: AgentState::Inactive,
        current_activation: None,
        latest_activation: latest.subagent_id.clone(),
        observation: latest.observation.clone(),
    };
    projection.bootstrap_agents(&[(owner, latest.clone())]);
    let (snapshot, cursor) = projection.snapshot().unwrap();
    assert_eq!(cursor, RuntimeClientCursor::new(0));
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents[0].activation_id, latest.subagent_id);
    assert_eq!(snapshot.agents[0].state, AgentState::Inactive);
    let mut next = latest.clone();
    next.subagent_id = crate::runtime::identity::SubagentId::new("activation-4");
    next.state = SubagentState::Running;
    next.settlement = crate::runtime::subagent::SubagentSettlement::default();
    projection.apply(owner_observation(next.clone(), AgentState::Active));
    let (resumed, _) = projection.snapshot().unwrap();
    assert_eq!(resumed.agents.len(), 1);
    assert_eq!(resumed.agents[0].agent_id, latest.child_agent_id);
    assert_eq!(resumed.agents[0].current_activation, Some(next.subagent_id));
}

#[test]
fn workflow_activations_do_not_become_continuable_agent_rows() {
    let mut projection = projection();
    let mut workflow =
        agent_activation_snapshot(crate::runtime::subagent::SubagentObservation::default());
    workflow.ownership = crate::events::types::SubagentOwnershipKind::Workflow;
    projection.apply(ConversationObservation::SubagentLifecycle {
        agent: None,
        snapshot: workflow.clone(),
    });
    projection.apply(ConversationObservation::SubagentActivity(workflow));
    let (snapshot, cursor) = projection.snapshot().unwrap();
    assert!(snapshot.agents.is_empty());
    assert_eq!(cursor, RuntimeClientCursor::new(0));
}

#[test]
fn t20_replay_byte_budget_covers_every_event_and_evicts_in_cursor_order() {
    use super::RUNTIME_CLIENT_REPLAY_BYTES;
    let mut projection = projection();
    projection.replay_limit = super::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT;
    let event = |bytes: usize| RuntimeClientEvent::BlockContentAdvanced {
        position: 0,
        attempt_id: attempt(),
        message_id: MessageId::new("message"),
        block_index: ContentBlockIndex::new(0),
        kind: "text".into(),
        base_length: 0,
        end_length: bytes as u64,
        content_revision: 1,
        inline_delta: Some("x".repeat(bytes)),
        content_ref: crate::observation::content::ContentRef {
            conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            source: crate::observation::content::ContentSource::LivePublicationPrefix {
                component: crate::observation::content::PublicationContentComponent::Payload,
                stream_id: crate::runtime::identity::PublicationStreamId::new("stream"),
                message_id: MessageId::new("message"),
                attempt_id: attempt(),
                turn_id: crate::runtime::identity::TurnId::new("turn"),
                publication_request_id: crate::runtime::identity::RequestId::new("request"),
                block_index: ContentBlockIndex::new(0),
            },
            representation: crate::observation::content::ContentRepresentation::Utf8Text,
            version: 1,
            content_revision: 1,
            readable_end: bytes as u64,
        },
    };
    for _ in 0..600 {
        projection.publish(event(32 * 1024));
        assert!(projection.replay_bytes <= RUNTIME_CLIENT_REPLAY_BYTES);
        assert_eq!(projection.replay.len(), projection.replay_sizes.len());
    }
    assert!(
        projection.replay.len() < 600,
        "text events must consume the byte budget"
    );
    let first = projection.replay.front().unwrap().0.get();
    for (index, (cursor, _)) in projection.replay.iter().enumerate() {
        assert_eq!(cursor.get(), first + u64::try_from(index).unwrap());
    }
    let actual: usize = projection
        .replay
        .iter()
        .map(|(_, event)| serde_json::to_vec(event).unwrap().len())
        .sum();
    assert_eq!(actual, projection.replay_bytes);
    projection.publish(event(RUNTIME_CLIENT_REPLAY_BYTES + 1));
    assert!(
        projection
            .replay
            .back()
            .unwrap()
            .1
            .facts()
            .iter()
            .any(|event| matches!(event, RuntimeClientEvent::CheckpointAdvanced { .. }))
    );
    assert!(projection.replay_bytes <= RUNTIME_CLIENT_REPLAY_BYTES);
    assert!(projection.replay.iter().all(|(_, event)| {
        crate::observation::bounded::bounded_bytes(
            event,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES,
        )
        .is_some()
    }));
}
#[test]
fn t14_t20_retained_cut_lookup_uses_exact_metadata_and_bounded_cursor_index() {
    let mut projection = projection();
    projection.replay_limit = 1;
    let mut cut = projection.checkpoint_read_cut();
    cut.revision = 9_001;
    projection.cursor = RuntimeClientCursor::new(1);
    projection.retain_replay(
        RuntimeClientEvent::ProjectionTransition {
            events: Vec::new(),
            metadata: super::super::checkpoint::ProjectionMetadata {
                read_cut: cut.clone(),
                collections: Vec::new(),
                content: Vec::new(),
            },
        },
        1,
    );
    assert!(projection.validate_checkpoint_cut(&cut).is_ok());
    let mut tampered = cut.clone();
    tampered.through += 1;
    assert!(matches!(
        projection.validate_checkpoint_cut(&tampered),
        Err(super::RuntimeClientError::ReadCutExpired)
    ));
    let mut checkpoint = projection.checkpoint_candidate().unwrap();
    checkpoint.read_cut = cut.clone();
    checkpoint.read_cut.revision = 9_002;
    let replacement_cut = checkpoint.read_cut.clone();
    projection.cursor = RuntimeClientCursor::new(2);
    projection.retain_replay(
        RuntimeClientEvent::CheckpointAdvanced {
            transition: "retained replacement".into(),
            checkpoint: Box::new(checkpoint),
        },
        1,
    );
    assert!(projection.validate_checkpoint_cut(&replacement_cut).is_ok());
    assert!(matches!(
        projection.validate_checkpoint_cut(&cut),
        Err(super::RuntimeClientError::ReadCutExpired)
    ));
    assert_eq!(projection.replay_cut_cursors.len(), 1);
    assert_eq!(
        projection.replay_cut_cursors.get(&9_002),
        Some(&RuntimeClientCursor::new(2))
    );
}

#[test]
fn t06_t14_oversized_transition_is_one_foldable_checkpoint_and_converges() {
    let (mut projection, store) = initialized_source_projection();
    let initial = projection.checkpoint_candidate().unwrap();
    let message = MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
        id: MessageId::new("large-canonical"),
        content: vec![AssistantContentBlock::Text(
            crate::message::content::TextBlock {
                text: "🙂\"".repeat(400_000),
            },
        )],
    });
    let receipt = store.append_canonical(&message).unwrap();
    projection.begin_publication();
    projection.apply(ConversationObservation::CanonicalCommitted {
        attempt_id: None,
        receipt: receipt.clone(),
    });
    let mut draft = projection.finish_publication().unwrap();
    // Inject the actual original C value only into its encoded candidate.
    // Normal producers already defer this body; the source cut, finite
    // state and reserved cursor still come from that exact native commit.
    draft.events = vec![RuntimeClientEvent::MessageCommitted {
        evicted_message_id: None,
        attempt_id: None,
        message,
        transcript_cursor: receipt.transcript_cursor.map(Into::into),
    }];
    let encoded = draft.encode().unwrap();
    assert!(encoded.bytes <= crate::observation::bounded::CHECKPOINT_TARGET_BYTES);
    projection.install_publication(encoded);
    assert_eq!(projection.cursor.get(), initial.cursor.get() + 1);
    let (cursor, event) = projection.replay.back().unwrap();
    let RuntimeClientEvent::ProjectionTransition { events, metadata } = event else {
        panic!("one complete native transition");
    };
    let [
        RuntimeClientEvent::CheckpointAdvanced {
            transition,
            checkpoint,
        },
    ] = events.as_slice()
    else {
        panic!("one bounded authoritative replacement");
    };
    assert_eq!(transition, "projection_transition");
    assert_eq!(checkpoint.cursor, *cursor);
    let mut folded = checkpoint.as_ref().clone();
    folded.read_cut.clone_from(&metadata.read_cut);
    folded.content.clone_from(&metadata.content);
    folded.collections.clone_from(&metadata.collections);
    assert_eq!(
        folded,
        projection.checkpoint_candidate().unwrap(),
        "complete replacement preserves all cut, domain, identity, frontier, and settlement fields"
    );
    assert!(
        crate::observation::bounded::bounded_bytes(
            event,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES
        )
        .is_some()
    );
    let payload = metadata
        .content
        .iter()
        .find(|binding| {
            matches!(
                binding.content_ref.source,
                crate::observation::content::ContentSource::CanonicalMessageBlock {
                    component: crate::observation::content::CanonicalContentComponent::Payload,
                    ..
                }
            )
        })
        .expect("original native canonical payload remains readable");
    assert_eq!(payload.content_ref.readable_end, 2_000_000);
    let cursor = *cursor;
    let (subscriber, _) = projection.subscribe(initial.cursor).unwrap();
    let SubscriberPoll::Event(observed) = projection.poll_subscriber(subscriber) else {
        panic!("event remains retained and serviceable")
    };
    assert_eq!(observed.cursor, cursor);
}

#[test]
fn t19_atomic_canonical_batch_advances_its_exact_transcript_interval() {
    use crate::durable::SqliteConversationStore;
    let mut projection = projection();
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        projection.snapshot.conversation_id.clone(),
        &directory.path().join("history.sqlite"),
    )
    .unwrap();
    projection
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    let messages = (0..73)
        .map(|index| {
            MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
                id: MessageId::new(format!("canonical-batch-{index}")),
                content: Vec::new(),
            })
        })
        .collect::<Vec<_>>();
    let receipts = store.append_canonical_batch(&messages).unwrap();
    assert_eq!(receipts[0].observation.transcript_count, 73);
    let cursor = projection.cursor;
    for receipt in receipts {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt,
        });
    }
    assert_eq!(projection.cursor.get(), cursor.get() + 1);
    assert_eq!(projection.transcript_prefix, 73);
    projection.validate_source_prefix().unwrap();
    assert_eq!(projection.snapshot.message_descriptors.len(), 8);
    let read = projection.checkpoint_read_cut().source_cut.unwrap();
    assert_eq!(read.read.transcript, 73);
    assert_eq!(read.transcript_total, 73);
}

fn publish_source_fact(
    projection: &mut RuntimeClientProjection,
    fold: impl FnOnce(&mut RuntimeClientProjection),
) {
    projection.begin_publication();
    projection.publication_commit_scope(true, true);
    fold(projection);
    projection.publication_commit_scope(false, true);
    if let Some(draft) = projection.finish_publication() {
        projection.install_publication(draft.encode().unwrap());
    }
}

mod metadata_root_tests;

mod prepared_trace_tests;

mod cached_trace_tests;

fn initialized_source_projection() -> (
    RuntimeClientProjection,
    crate::durable::SqliteConversationStore,
) {
    let mut projection = projection();
    let store = crate::durable::SqliteConversationStore::in_memory(
        projection.snapshot.conversation_id.clone(),
    )
    .unwrap();
    projection
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    projection.initialize_publication_view();
    (projection, store)
}

#[test]
fn worker_read_candidate_keeps_native_cut_without_capturing_checkpoint() {
    let (mut projection, store) = initialized_source_projection();
    let published = projection.checkpoint_seed();
    assert!(!published.trace_head.prepared());
    projection.request_read_domains_for_test();
    let candidate = projection.read_domain_candidate().unwrap();
    assert_eq!(candidate, projection.read_domain_fence());
    assert!(
        !published.trace_head.prepared(),
        "worker admission must not capture or fit a published checkpoint"
    );
    let read = candidate.read(&store).unwrap();
    assert_eq!(
        read.transcript.source_cut.as_ref(),
        candidate.source_cut.as_ref()
    );
    assert!(projection.install_read_domains(&candidate, Ok(read), false));
    assert!(!published.trace_head.prepared());
    assert!(projection.read_domain_candidate().is_none());
    // The explicit snapshot test API still captures its own published view.
    let snapshot = projection.snapshot_candidate().unwrap();
    assert!(published.trace_head.prepared());
    assert_eq!(snapshot.cursor, published.cursor);
    assert_eq!(snapshot.read_cut, candidate);
}

#[test]
fn t14_t20_initial_cut_expires_with_its_replay_cursor() {
    let (mut projection, store) = initialized_source_projection();
    projection.replay_limit = 1;
    let initial = projection.checkpoint_candidate().unwrap().read_cut;
    assert_eq!(
        projection.replay_bytes,
        crate::observation::bounded::CHECKPOINT_TARGET_BYTES
    );
    for ordinal in 1..=2 {
        let receipt = store
            .append_canonical(&compactable_user(&format!("initial-cut-{ordinal}")))
            .unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::CanonicalCommitted {
                attempt_id: None,
                receipt,
            });
        });
        assert!(projection.replay_bytes <= super::RUNTIME_CLIENT_REPLAY_BYTES);
        if ordinal == 1 {
            assert!(projection.validate_checkpoint_cut(&initial).is_ok());
            let mut tampered = initial.clone();
            tampered.through += 1;
            assert!(projection.validate_checkpoint_cut(&tampered).is_err());
        }
    }
    assert!(projection.initial_read_cut.is_none());
    assert!(
        !projection
            .replay_cut_cursors
            .contains_key(&initial.revision)
    );
    assert_eq!(
        projection.replay_bytes,
        projection.replay_sizes.iter().sum::<usize>()
    );
    assert!(matches!(
        projection.validate_checkpoint_cut(&initial),
        Err(super::RuntimeClientError::ReadCutExpired)
    ));
}

#[test]
fn t14_t20_initial_cut_metadata_is_charged_to_the_replay_byte_budget() {
    let (mut projection, _store) = initialized_source_projection();
    let initial = projection.checkpoint_candidate().unwrap().read_cut;
    for cursor in 1..=1_000 {
        // Stress real schema-aware record weights at the existing replay
        // boundary; no source or execution state is changed by this probe.
        let mut seed = projection.capture_working_seed();
        seed.cursor = RuntimeClientCursor::new(cursor);
        seed.read_cut.revision = cursor;
        let event = RuntimeClientEvent::CheckpointAdvanced {
            transition: "bounded replay weight ".repeat(1_500),
            checkpoint: Box::new(seed.checkpoint_candidate().unwrap()),
        };
        let bytes = super::encoded_event_bytes(
            &event,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES,
        )
        .unwrap()
        .len();
        projection.install_publication(super::EncodedPublication {
            seed: Arc::new(seed),
            event,
            bytes,
            sources: projection.domain_sources.clone(),
        });
        assert!(projection.replay_bytes <= super::RUNTIME_CLIENT_REPLAY_BYTES);
        let reservation = usize::from(projection.initial_read_cut.is_some())
            * crate::observation::bounded::CHECKPOINT_TARGET_BYTES;
        assert_eq!(
            projection.replay_bytes,
            projection.replay_sizes.iter().sum::<usize>() + reservation
        );
        if projection.initial_read_cut.is_none() {
            assert!(projection.earliest_serviceable().get() > 0);
            assert!(projection.validate_checkpoint_cut(&initial).is_err());
            return;
        }
    }
    panic!("encoded byte pressure must retire the initial cut before the count limit");
}

#[test]
fn t14_native_admission_publishes_frozen_model_before_execution_starts() {
    let (mut projection, _store) = initialized_source_projection();
    let initial = projection.checkpoint_candidate().unwrap();
    let session = crate::scripted_suites::support::model::scripted_session_model(Arc::new(
        crate::scripted_suites::support::model::NullAdapter,
    ));
    let model = crate::model::session::AttemptModelReadSource::new(session.snapshot().unwrap());
    model.prepare();
    let frozen = model.inline();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::AttemptAdmitted {
            read_image_active: false,
            attempt_id: attempt(),
            model,
            resource_revision: crate::runtime::identity::RuntimeResourceRevision::new(7),
            approval_mode: crate::runtime::types::ApprovalMode::Policy,
        });
    });
    let current = projection.checkpoint_candidate().unwrap();
    assert_eq!(current.cursor.get(), initial.cursor.get() + 1);
    assert_eq!(current.read_cut.through, initial.read_cut.through);
    let admitted = current.snapshot.attempt.as_ref().unwrap();
    assert_eq!(admitted.phase, RuntimeClientAttemptPhase::Admitted);
    assert_eq!(admitted.model.as_deref(), frozen.as_ref());
    assert_eq!(admitted.turn, 0);
    assert!(admitted.foreground.is_empty());
    let event = &projection.replay.back().unwrap().1;
    assert!(
        !event
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
    );
    assert!(event.facts().iter().any(|fact| matches!(fact,
        RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. }
            if checkpoint.snapshot.attempt == current.snapshot.attempt)));
}

fn source_tool_event(
    store: &dyn crate::durable::ConversationStore,
    event: RuntimeEvent,
    identity: &str,
) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        event_id: EventId::new(identity),
        sequence: 0,
        conversation_id: store.conversation_id().clone(),
        attempt_id: Some(attempt()),
        turn_id: Some(TurnId::new("1")),
        timestamp: chrono::DateTime::UNIX_EPOCH,
        tool_occurrence: None,
        observation: None,
        event,
    }
}

fn native_running_tool_fixture() -> (
    RuntimeClientProjection,
    crate::durable::SqliteConversationStore,
    ToolCall,
    crate::durable::observation::ObservationEventWitness,
) {
    let (mut projection, store) = initialized_source_projection();
    let started = append_source_attempt_start(&store, "progress-attempt-start");
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(started.observation.as_deref().unwrap());
    });
    store
        .mark_observation_direct(started.sequence, started.sequence)
        .unwrap();
    let call = ToolCall {
        id: ToolCallId::new("progress-call"),
        tool_id: ToolId::new("progress-tool"),
        name: "progress_tool".into(),
        arguments: serde_json::json!({}),
    };
    let message = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("progress-proposal"),
        content: vec![AssistantContentBlock::ToolCall(call.clone())],
    });
    let (committed, _) = store
        .append_canonical_with_event(
            &message,
            source_tool_event(
                &store,
                RuntimeEvent::AssistantMessageCommitted {
                    message_id: message.id().clone(),
                },
                "progress-proposal-commit",
            ),
        )
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(committed.observation.as_deref().unwrap());
    });
    store
        .mark_observation_direct(committed.sequence, committed.sequence)
        .unwrap();
    store.mark_canonical_transcript_ready(1, 1).unwrap();
    // The display baseline is the actual canonical assembled occurrence;
    // execution state below comes exclusively from its native Started J.
    projection.snapshot.attempt.as_mut().unwrap().foreground =
        projection.snapshot.message_descriptors[0]
            .tool_calls
            .clone();
    let started = store
        .append_event(source_tool_event(
            &store,
            RuntimeEvent::ToolExecutionStarted {
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
            },
            "progress-tool-start",
        ))
        .unwrap();
    let prepared = store
        .observation_event_witness(started.sequence, started.sequence)
        .unwrap()
        .unwrap();
    assert!(!prepared.native_direct);
    assert!(prepared.tool_occurrence.is_some());
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(started.observation.as_deref().unwrap());
    });
    store
        .mark_observation_direct(started.sequence, started.sequence)
        .unwrap();
    (projection, store, call, prepared)
}

#[test]
fn t14_t19_prepared_started_witness_preserves_exact_live_progress_and_terminal_facts() {
    let (mut projection, store, call, prepared) = native_running_tool_fixture();
    let historical = projection.checkpoint_seed();
    let progress = ToolProgress {
        message: Some("phase 1".into()),
        ..ToolProgress::default()
    };
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::ToolOccurrenceProof {
            occurrence: prepared.tool_occurrence.clone().unwrap(),
            observation: Box::new(ConversationObservation::ToolProgress {
                attempt_id: attempt(),
                tool_call_id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                progress: progress.clone(),
            }),
        });
    });
    let before = projection.checkpoint_candidate().unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::JournalBatch {
            through: Some(prepared.sequence),
            observations: vec![ConversationObservation::JournalFact(prepared.clone())],
        });
    });
    assert_eq!(projection.checkpoint_candidate().unwrap(), before);
    let seed = projection.checkpoint_seed();
    let slot = &before.snapshot.attempt.as_ref().unwrap().foreground[0];
    assert!(
        matches!(&slot.state, ForegroundToolState::Running { progress: Some(value), .. } if value == &progress)
    );
    let mut page = super::super::tool_observation::page(
        &store,
        &before.read_cut,
        "attempt.foreground",
        &slot.occurrence.locator,
        0,
        32,
    )
    .unwrap();
    historical.retain_foreground_progress(&mut page).unwrap();
    let durable: crate::observation::tool::ForegroundToolExecution =
        crate::observation::bounded::decode_domain_value(page.items[0].value.clone().unwrap())
            .unwrap();
    assert!(matches!(
        durable.state,
        ForegroundToolState::Running { progress: None, .. }
    ));
    seed.retain_foreground_progress(&mut page).unwrap();
    let live: crate::observation::tool::ForegroundToolExecution =
        crate::observation::bounded::decode_domain_value(page.items[0].value.clone().unwrap())
            .unwrap();
    assert_eq!(live, *slot);
    let terminal = store
        .append_event(source_tool_event(
            &store,
            RuntimeEvent::ToolExecutionCompleted {
                tool_call_id: call.id,
                tool_id: call.tool_id,
                result: success_result(),
            },
            "progress-tool-completed",
        ))
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(terminal.observation.as_deref().unwrap());
    });
    let settled = projection.checkpoint_candidate().unwrap();
    assert!(settled.cursor > before.cursor);
    assert!(matches!(
        settled.snapshot.attempt.as_ref().unwrap().foreground[0].state,
        ForegroundToolState::Settled { .. }
    ));
    assert!(
        projection
            .replay
            .back()
            .unwrap()
            .1
            .facts()
            .iter()
            .any(|fact| matches!(fact, RuntimeClientEvent::ToolExecutionSettled { .. }))
    );
}

fn append_source_attempt_start(
    store: &dyn crate::durable::ConversationStore,
    identity: &str,
) -> RuntimeEventEnvelope {
    store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(identity),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(attempt()),
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            tool_occurrence: None,
            observation: None,
            event: RuntimeEvent::AttemptStarted {
                attempt_id: attempt(),
            },
        })
        .unwrap()
}

/// An earlier native owner can delay release indefinitely without creating
/// a sparse skip map or granting future J/C/pending source access.
#[test]
fn t14_t20_native_direct_receipts_wait_for_the_exact_earlier_source_prefix() {
    let (mut projection, store) = initialized_source_projection();
    let initial = projection.checkpoint_read_cut();
    let earlier = append_source_attempt_start(&store, "held-native-owner");
    let mut goal = None;
    for index in 0..73 {
        let write = goal.map_or_else(
            || crate::goal::GoalWrite::Create {
                objective: "native goal 0".into(),
                budget: 3,
                origin: crate::goal::GoalOrigin::RuntimeControl,
            },
            |expected| crate::goal::GoalWrite::Mutate {
                expected,
                mutation: crate::goal::GoalMutation::Edit {
                    objective: format!("native goal {index}"),
                },
            },
        );
        let (result, receipt) = store.write_goal(write).unwrap();
        goal = Some(result.unwrap().reference);
        let receipt = receipt.unwrap();
        let native = receipt.native.as_ref().unwrap();
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::GoalChanged(receipt.view.clone()));
            projection.apply_native_batch(native);
        });
        assert!(projection.native_receipt_installation().is_installed());
        let (first, last) = native.journal_range.unwrap();
        store.mark_observation_direct(first, last).unwrap();
        assert_eq!(store.observation_ready_frontier(0, last).unwrap(), 0);
        assert_eq!(projection.checkpoint_read_cut().through, 0);
        assert_eq!(projection.checkpoint_read_cut(), initial);
        assert!(projection.working_read_cut().source_cut.is_none());
        assert!(
            projection.snapshot_ref().goal.is_none(),
            "published C retains its earlier Goal state"
        );
        assert_eq!(
            projection
                .snapshot
                .goal
                .as_ref()
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .reference,
            goal.clone().unwrap()
        );
    }
    let latest = projection.source_cut.clone().unwrap();
    assert_eq!(projection.native_journal_highwater, 74);
    assert_eq!(
        projection
            .snapshot
            .goal
            .as_ref()
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .objective,
        "native goal 72"
    );
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(earlier.observation.as_ref().unwrap());
    });
    assert_eq!(
        projection.source_cut.as_ref().unwrap(),
        &latest,
        "the delayed owner's cut cannot roll back the later installed source"
    );
    store
        .mark_observation_direct(earlier.sequence, earlier.sequence)
        .unwrap();
    assert_eq!(store.observation_ready_frontier(0, 74).unwrap(), 74);
    for sequence in 1..=74 {
        let witness = store
            .observation_event_witness(sequence, 74)
            .unwrap()
            .unwrap();
        assert!(witness.native_direct);
        publish_source_fact(&mut projection, |projection| {
            projection.apply(ConversationObservation::JournalBatch {
                through: Some(sequence),
                observations: vec![ConversationObservation::JournalFact(witness)],
            });
        });
    }
    assert_released_goal_prefix(&projection, &goal.unwrap());
}

fn assert_released_goal_prefix(projection: &RuntimeClientProjection, goal: &crate::goal::GoalRef) {
    assert_eq!(projection.checkpoint_read_cut().through, 74);
    assert_eq!(
        projection
            .checkpoint_read_cut()
            .source_cut
            .as_ref()
            .unwrap()
            .read
            .journal,
        74
    );
    assert_eq!(
        projection
            .snapshot_ref()
            .goal
            .as_ref()
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .reference,
        *goal
    );
    assert_eq!(
        projection
            .snapshot_ref()
            .attempt
            .as_ref()
            .unwrap()
            .attempt_id,
        Some(attempt())
    );
    assert!(projection.prepared_journal.is_empty());
    assert!(projection.prepared_outcomes.is_empty());
}

#[test]
fn t14_t20_no_j_canonical_and_pending_cuts_do_not_authorize_a_held_journal_owner() {
    let (mut projection, store) = initialized_source_projection();
    let published = projection.checkpoint_read_cut();
    let earlier = append_source_attempt_start(&store, "held-before-no-j-c");
    let message = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("native-no-j-c"),
        content: Vec::new(),
    });
    let canonical = store.append_canonical(&message).unwrap();
    assert!(canonical.observation.journal_range.is_none());
    let pending = store
        .accept_inbound(crate::durable::inbox::InboundDraft {
            message_id: None,
            source: crate::message::types::UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            content: vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "later pending owner".into(),
                },
            )],
            timestamp: chrono::DateTime::UNIX_EPOCH,
            correlation: None,
        })
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply_pending_receipt(pending.pending_observation.as_deref());
    });
    let latest = projection.source_cut.clone().unwrap();
    assert_eq!(
        latest.read.mutation_revision,
        pending.read_cut.mutation_revision
    );
    assert_eq!(projection.native_journal_highwater, earlier.sequence);
    assert_eq!(projection.checkpoint_read_cut().through, 0);
    assert_eq!(projection.checkpoint_read_cut(), published);
    assert!(projection.working_read_cut().source_cut.is_none());
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(earlier.observation.as_ref().unwrap());
    });
    assert_eq!(projection.source_cut.as_ref().unwrap(), &latest);
    assert_eq!(
        projection.checkpoint_read_cut(),
        published,
        "uninstalled C cannot mutate the published seed"
    );
    assert!(
        projection.working_read_cut().source_cut.is_none(),
        "uninstalled C still has its own owner"
    );
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: canonical,
        });
    });
    store
        .mark_observation_direct(earlier.sequence, earlier.sequence)
        .unwrap();
    store.mark_canonical_transcript_ready(1, 1).unwrap();
    store
        .mark_observation_transcript_ready(pending.read_cut.transcript)
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::TranscriptSourceReady(
            store.transcript_source_ready().unwrap(),
        ));
    });
    let cut = projection.checkpoint_read_cut();
    let source = cut.source_cut.unwrap();
    assert_eq!(source, store.observation_read_cut().unwrap());
    assert_eq!(
        source.read.mutation_revision,
        pending.read_cut.mutation_revision
    );
    assert_eq!(source.ledger_through, 1);
    assert_eq!(source.read.transcript, 2);
    assert_eq!(projection.snapshot_ref().inbound.pending.len(), 1);
    assert_eq!(projection.snapshot_ref().message_descriptors.len(), 1);
    assert_eq!(
        projection
            .snapshot_ref()
            .attempt
            .as_ref()
            .unwrap()
            .attempt_id,
        Some(attempt())
    );
}

#[test]
fn t14_t19_complete_canonical_head_survives_an_earlier_delayed_callback() {
    let (mut projection, store) = initialized_source_projection();
    let initial = projection.checkpoint_read_cut();
    let first = store
        .append_canonical(&MessageBlock::User(
            crate::message::types::UserMessageBlock {
                id: MessageId::new("first-native-user"),
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: Some(chrono::DateTime::UNIX_EPOCH),
                content: vec![crate::message::types::UserContentBlock::Text(
                    crate::message::content::TextBlock {
                        text: "first native preview".into(),
                    },
                )],
            },
        ))
        .unwrap();
    let second = store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("second-native-c"),
            content: Vec::new(),
        }))
        .unwrap();
    assert_eq!(
        second.observation.head.len(),
        2,
        "later native transaction captures complete global canonical head"
    );
    assert_eq!(
        second.observation.opening_preview.as_deref(),
        Some("first native preview")
    );
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: second,
        });
    });
    assert_eq!(
        projection.checkpoint_read_cut(),
        initial,
        "a global head does not release the earlier owned T"
    );
    assert_eq!(projection.snapshot.message_descriptors.len(), 2);
    let latest = projection.source_cut.clone();
    store.mark_canonical_transcript_ready(2, 2).unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: first,
        });
    });
    assert_eq!(
        projection.source_cut, latest,
        "earlier complete view cannot roll back newer cut"
    );
    assert_eq!(
        projection.snapshot.message_descriptors.len(),
        2,
        "earlier callback does not duplicate the complete head"
    );
    store.mark_canonical_transcript_ready(1, 1).unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::TranscriptSourceReady(
            store.transcript_source_ready().unwrap(),
        ));
    });
    assert_eq!(projection.cursor.get(), initial.revision + 1);
    assert_eq!(
        projection.checkpoint_read_cut().source_cut.unwrap(),
        store.observation_read_cut().unwrap()
    );
    assert_eq!(
        projection
            .snapshot
            .messages
            .iter()
            .map(|message| message.id().as_str())
            .collect::<Vec<_>>(),
        ["first-native-user", "second-native-c"]
    );
}

fn assert_canonical_header_fact(
    projection: &mut RuntimeClientProjection,
    header: &crate::durable::observation::MessageHeader,
    owns_header: bool,
) {
    assert!(
        !projection
            .fold_canonical_header(None, header.clone(), header.transcript_cursor, owns_header)
            .is_empty()
    );
}

#[test]
fn t14_t19_complete_canonical_head_omits_only_exact_unchanged_rows() {
    let (mut projection, store) = initialized_source_projection();
    let [first, second] = canonical_capture_receipts(&store);
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: first,
        });
    });
    store.mark_canonical_transcript_ready(1, 1).unwrap();
    let cursor = projection.cursor;
    let earlier = second.observation.head[0].clone();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: second,
        });
    });
    let facts = projection
        .replay
        .iter()
        .filter(|(at, _)| *at > cursor)
        .flat_map(|(_, event)| event.facts())
        .collect::<Vec<_>>();
    assert_eq!(
        facts
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::MessageDescriptorCommitted { .. }))
            .count(),
        1
    );
    assert_eq!(
        facts
            .iter()
            .filter(|event| matches!(event, RuntimeClientEvent::MessageCommitted { .. }))
            .count(),
        1
    );
    assert_eq!(projection.snapshot.message_descriptors.len(), 2);
    assert!(
        projection
            .fold_canonical_header(None, earlier.clone(), earlier.transcript_cursor, false)
            .is_empty()
    );
    let mut changed_body = earlier.clone();
    let Some(MessageBlock::User(body)) = &mut changed_body.display else {
        panic!("native User source header")
    };
    let UserContentBlock::Text(text) = &mut body.content[0] else {
        panic!("native User text")
    };
    text.text.push_str(" changed");
    assert_canonical_header_fact(&mut projection, &changed_body, false);
    assert_canonical_header_fact(&mut projection, &earlier, false);
    let mut refined = earlier.clone();
    refined.content[0].content_ref.readable_end += 1;
    assert_canonical_header_fact(&mut projection, &refined, false);
    assert_canonical_header_fact(&mut projection, &earlier, false);
    assert!(
        projection
            .fold_canonical_header(None, earlier.clone(), earlier.transcript_cursor, false)
            .is_empty()
    );
    assert_canonical_header_fact(&mut projection, &earlier, true);
}

fn canonical_capture_wire<T: serde::Serialize + schemars::JsonSchema>(
    value: &T,
) -> serde_json::Value {
    let mut bytes = Vec::new();
    crate::observation::structural_json::to_writer(&mut bytes, value).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn canonical_capture_case(
    name: &str,
    initial: crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    projection: &RuntimeClientProjection,
) -> serde_json::Value {
    use crate::app_server::protocol::{
        AttachmentTarget, JsonRpcVersion, MethodResult, Notification, NotificationMethod,
        RequestId as WireId, Response, Success,
    };
    let expected = projection.checkpoint_candidate().unwrap();
    let target = AttachmentTarget {
        session_id: crate::local_runtime::session::SessionId::new(
            "ses_00000000-0000-7000-8000-000000000421",
        ),
        conversation_id: initial.snapshot.conversation_id.clone(),
        runtime_incarnation: serde_json::from_str("1").unwrap(),
        attachment_id: crate::runtime_client::AttachmentId::new("canonical-capture"),
    };
    let events = projection
        .replay
        .iter()
        .filter(|(cursor, _)| *cursor > initial.cursor)
        .map(|(cursor, event)| {
            assert!(
                !event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
            );
            canonical_capture_wire(&Notification {
                jsonrpc: JsonRpcVersion::V2,
                notification: NotificationMethod::Event {
                    target: target.clone(),
                    registration_id: "canonical-capture".into(),
                    cursor: *cursor,
                    event: Box::new(event.clone()),
                },
            })
        })
        .collect::<Vec<_>>();
    assert!(!events.is_empty());
    let checkpoint = |value| {
        canonical_capture_wire(&Response::Success(Box::new(Success {
            jsonrpc: JsonRpcVersion::V2,
            id: WireId::String("canonical-capture".into()),
            result: MethodResult::Checkpoint {
                checkpoint: Box::new(value),
            },
        })))
    };
    serde_json::json!({
        "name": name, "initial": checkpoint(initial),
        "events": events, "expected": checkpoint(expected),
    })
}

fn canonical_capture_receipts(
    store: &dyn crate::durable::ConversationStore,
) -> [crate::durable::TranscriptCommitReceipt; 2] {
    let first = store
        .append_canonical(&MessageBlock::User(UserMessageBlock {
            id: MessageId::new("boundary-user"),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: Some(chrono::DateTime::UNIX_EPOCH),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "boundary preview".into(),
            })],
        }))
        .unwrap();
    let second = store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("boundary-assistant"),
            content: Vec::new(),
        }))
        .unwrap();
    [first, second]
}

fn canonical_tool_result_capture_case() -> serde_json::Value {
    let (mut projection, store) = initialized_source_projection();
    let call = ToolCall {
        id: ToolCallId::new("canonical-boundary-call"),
        tool_id: ToolId::new("canonical-boundary-tool"),
        name: "boundary_tool".into(),
        arguments: serde_json::json!({}),
    };
    let assistant = MessageBlock::Assistant(AssistantMessageBlock {
        id: MessageId::new("canonical-boundary-proposal"),
        content: vec![AssistantContentBlock::ToolCall(call.clone())],
    });
    let receipt = store.append_canonical(&assistant).unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt,
        });
    });
    store.mark_canonical_transcript_ready(1, 1).unwrap();
    let initial = projection.checkpoint_candidate().unwrap();
    let result = cancelled_result(
        CancellationReason::RuntimeShutdown,
        ToolCancellationPhase::BeforeStart,
    );
    let receipt = store
        .append_canonical(&MessageBlock::Tool(ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                assistant.id().clone(),
                ContentBlockIndex::new(0),
            ),
            id: MessageId::new("canonical-boundary-result"),
            tool_call_id: call.id,
            tool_id: call.tool_id,
            result: result.clone(),
        }))
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt,
        });
    });
    let descriptor = &projection.snapshot.message_descriptors[0];
    let slot = &descriptor.tool_calls[0];
    assert_eq!(slot.committed_result.as_ref(), Some(&result));
    assert!(matches!(slot.state, ForegroundToolState::Assembled { .. }));
    assert!(slot.result_metadata_ref.is_some());
    assert!(
        descriptor
            .collections
            .iter()
            .any(|frontier| frontier.domain == "messages.ledger:2.blocks" && frontier.total == 0)
    );
    canonical_capture_case(
        "tool result C metadata before terminal J",
        initial,
        &projection,
    )
}

/// App-encoded native checkpoints isolate canonical publication from the
/// asynchronous transcript read domain; no client fold produces the oracle.
#[test]
fn t14_native_canonical_boundary_independent_checkpoint_capture() {
    let (mut projection, store) = initialized_source_projection();
    let initial = projection.checkpoint_candidate().unwrap();
    let [first, second] = canonical_capture_receipts(&store);
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: first,
        });
    });
    let first_case = canonical_capture_case(
        "first canonical C before transcript refresh",
        initial,
        &projection,
    );
    let initial = projection.checkpoint_candidate().unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: second,
        });
    });
    let second_case = canonical_capture_case(
        "later canonical C with empty block domain",
        initial,
        &projection,
    );
    let (mut delayed, store) = initialized_source_projection();
    let initial = delayed.checkpoint_candidate().unwrap();
    let [first, second] = canonical_capture_receipts(&store);
    publish_source_fact(&mut delayed, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: second,
        });
    });
    assert_eq!(delayed.checkpoint_candidate().unwrap(), initial);
    store.mark_canonical_transcript_ready(2, 2).unwrap();
    publish_source_fact(&mut delayed, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: first,
        });
    });
    store.mark_canonical_transcript_ready(1, 1).unwrap();
    publish_source_fact(&mut delayed, |projection| {
        projection.apply(ConversationObservation::TranscriptSourceReady(
            store.transcript_source_ready().unwrap(),
        ));
    });
    let delayed_case =
        canonical_capture_case("complete C2 head after delayed C1 owner", initial, &delayed);
    let tool_case = canonical_tool_result_capture_case();
    if let Ok(path) = std::env::var("RUSTX_CANONICAL_CAPTURE") {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::json!({
                "cases": [first_case, second_case, delayed_case, tool_case],
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn t14_t19_hidden_canonical_dependency_fences_a_native_audit_without_a_t_gap() {
    let fixture = crate::durable::SqliteConversationStore::hidden_context_audit_fixture();
    let mut projection = projection();
    projection.snapshot.conversation_id = fixture.before.read.conversation_id.clone();
    projection
        .initialize_descriptor_heads(fixture.store.as_ref(), fixture.before.clone())
        .unwrap();
    projection.set_inspection_frontier(fixture.before.read.journal);
    projection.initialize_publication_view();
    let initial = projection.checkpoint_read_cut();
    assert!(fixture.context.transcript_cursor.is_none());
    assert_eq!(fixture.audit.transcript_cursor.get(), 1);
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::AuditSettled {
            attempt_id: fixture.audit.summary.attempt_id.clone().unwrap(),
            receipt: fixture.audit.clone(),
        });
    });
    assert_eq!(
        projection.transcript_prefix, 1,
        "there is no hidden T hole masking the C dependency"
    );
    assert_eq!(projection.native_canonical_highwater, 1);
    assert_eq!(projection.canonical_observed_through, 0);
    assert_eq!(
        projection.checkpoint_read_cut(),
        initial,
        "audit cannot publish SourceCutNone across an uninstalled hidden C"
    );
    assert!(matches!(
        projection.validate_source_prefix(),
        Err(super::RuntimeClientError::ReadCutExpired)
    ));
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: fixture.context,
        });
    });
    assert_eq!(projection.cursor.get(), initial.revision + 1);
    assert_eq!(projection.snapshot.publication_settlements.len(), 1);
    assert_eq!(
        projection.checkpoint_read_cut().source_cut.unwrap(),
        fixture.store.observation_read_cut().unwrap()
    );
}

#[test]
fn t19_published_checkpoint_is_independent_of_pending_native_owner_work() {
    use crate::durable::SqliteConversationStore;
    let mut projection = projection();
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        projection.snapshot.conversation_id.clone(),
        &directory.path().join("history.sqlite"),
    )
    .unwrap();
    projection
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    projection.initialize_publication_view();
    let published = projection.checkpoint_seed();
    let cut = published.read_cut.clone();
    projection.observe_transcript_position(crate::durable::TranscriptCursor::new(2));
    assert!(
        projection.validate_source_prefix().is_err(),
        "working T has an unresolved owner"
    );
    projection.validate_published_source().unwrap();
    assert_eq!(projection.checkpoint_read_cut(), cut);
    assert_eq!(
        projection.checkpoint_seed().checkpoint_candidate().unwrap(),
        published.checkpoint_candidate().unwrap()
    );
    projection.publication_commit_scope(true, false);
    projection.publication_commit_scope(false, false);
    assert!(
        !projection.source_state.source_unavailable,
        "queued observations cannot invalidate the native source proof"
    );
    projection.validate_published_source().unwrap();
}

#[test]
fn t19_unobserved_transcript_position_fences_a_later_transition_until_its_owner_installs() {
    use crate::durable::ConversationStore;
    let (mut projection, store) = initialized_source_projection();
    let started = append_source_attempt_start(&store, "t-prefix-start");
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(started.observation.as_deref().unwrap());
    });
    store
        .mark_observation_direct(started.sequence, started.sequence)
        .unwrap();
    let earlier = store
        .append_event(RuntimeEventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new("held-terminal-t"),
            sequence: 0,
            conversation_id: store.conversation_id().clone(),
            attempt_id: Some(attempt()),
            turn_id: None,
            timestamp: chrono::DateTime::UNIX_EPOCH,
            tool_occurrence: None,
            observation: None,
            event: RuntimeEvent::AttemptFailed {
                attempt_id: attempt(),
                error: crate::events::types::AttemptFailure::Runtime {
                    error: crate::runtime::types::RuntimeError::Internal {
                        message: "native terminal".into(),
                    },
                },
            },
        })
        .unwrap();
    let later = store
        .append_canonical(&MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("later-native-c"),
            content: Vec::new(),
        }))
        .unwrap();
    let cursor = projection.cursor;
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: later,
        });
    });
    store.mark_canonical_transcript_ready(1, 1).unwrap();
    assert_eq!(store.transcript_source_ready().unwrap().prefix, 0);
    assert_eq!(
        projection.cursor, cursor,
        "unreleased T=1 fences the later native C"
    );
    assert!(matches!(
        projection.validate_source_prefix(),
        Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
    ));
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(earlier.observation.as_deref().unwrap());
    });
    store
        .mark_observation_direct(earlier.sequence, earlier.sequence)
        .unwrap();
    store.mark_observation_transcript_ready(1).unwrap();
    let proof = store.transcript_source_ready().unwrap();
    assert_eq!(proof.prefix, 2);
    publish_source_fact(&mut projection, |projection| {
        projection.apply(ConversationObservation::TranscriptSourceReady(proof));
    });
    assert_eq!(
        projection.cursor.get(),
        cursor.get() + 1,
        "the exact complete source prefix publishes once"
    );
    projection.validate_source_prefix().unwrap();
    let source = projection.checkpoint_read_cut().source_cut.unwrap();
    assert_eq!(source, store.observation_read_cut().unwrap());
    assert_eq!(source.read.transcript, 2);
    assert_eq!(source.read.journal, 2);
    assert_eq!(source.ledger_through, 1);
}

#[test]
fn t16_pending_adoption_receipt_releases_only_its_matching_mutation_fence() {
    use crate::durable::SqliteConversationStore;
    let mut projection = projection();
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteConversationStore::open(
        projection.snapshot.conversation_id.clone(),
        &directory.path().join("history.sqlite"),
    )
    .unwrap();
    projection
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    let mut pending_first = self::projection();
    pending_first
        .initialize_descriptor_heads(&store, store.observation_read_cut().unwrap())
        .unwrap();
    let accepted = store
        .accept_inbound(crate::durable::inbox::InboundDraft {
            message_id: None,
            source: crate::message::types::UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            content: vec![crate::message::types::UserContentBlock::Text(
                crate::message::content::TextBlock {
                    text: "native pending adoption".into(),
                },
            )],
            timestamp: chrono::Utc::now(),
            correlation: None,
        })
        .unwrap();
    projection.apply_pending_receipt(accepted.pending_observation.as_deref());
    pending_first.apply_pending_receipt(accepted.pending_observation.as_deref());
    let adopted = store.adopt_pending_batch(accepted.sequence, None).unwrap();
    pending_first.apply_pending_receipt(adopted.pending_observation.as_deref());
    for receipt in &adopted.canonical_receipts {
        pending_first.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: receipt.clone(),
        });
    }
    assert!(
        pending_first.pending_epoch_wait.is_none(),
        "native mailbox publishes its pending receipt before Agent canonical callbacks; that order cannot restore an already represented mutation fence"
    );
    for receipt in &adopted.canonical_receipts {
        projection.apply(ConversationObservation::CanonicalCommitted {
            attempt_id: None,
            receipt: receipt.clone(),
        });
    }
    assert_eq!(
        projection.pending_epoch_wait,
        Some(adopted.read_cut.mutation_revision)
    );
    projection.apply_pending_receipt(accepted.pending_observation.as_deref());
    assert_eq!(
        projection.pending_epoch_wait,
        Some(adopted.read_cut.mutation_revision),
        "an older pending receipt cannot release the adoption fence"
    );
    projection.apply_pending_receipt(adopted.pending_observation.as_deref());
    assert!(
        projection.pending_epoch_wait.is_none(),
        "the exact native adoption receipt releases its pending revision fence"
    );
    let native = adopted
        .adoption
        .as_ref()
        .unwrap()
        .observation
        .as_deref()
        .unwrap();
    publish_source_fact(&mut projection, |projection| {
        projection.apply_native_batch(native);
    });
    publish_source_fact(&mut pending_first, |projection| {
        projection.apply_native_batch(native);
    });
    assert!(!matches!(
        projection.validate_source_prefix(),
        Err(crate::runtime_client::RuntimeClientError::ReadCutExpired)
    ));
}

#[test]
fn t15_many_small_message_pages_advance_under_both_native_budgets() {
    let mut projection = projection();
    projection.snapshot.messages = (0..1000)
        .map(|index| {
            MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
                id: MessageId::new(format!("message-{index}")),
                content: vec![AssistantContentBlock::Text(
                    crate::message::content::TextBlock { text: "x".into() },
                )],
            })
        })
        .collect();
    let checkpoint = projection.checkpoint_candidate().unwrap();
    assert!(checkpoint.snapshot.messages.len() <= 32);
    assert!(
        crate::observation::bounded::bounded_bytes(
            &checkpoint,
            crate::observation::bounded::CHECKPOINT_TARGET_BYTES
        )
        .is_some()
    );
    let mut offset = 0;
    let mut identities = Vec::new();
    loop {
        let page = projection
            .checkpoint_page(&checkpoint.read_cut, "messages", offset, 32)
            .unwrap();
        assert!(page.items.len() <= 32);
        assert!(
            crate::observation::bounded::bounded_bytes(
                &page,
                crate::observation::bounded::CHECKPOINT_TARGET_BYTES
            )
            .is_some()
        );
        identities.extend(page.items.iter().map(|item| item.identity.clone()));
        match page.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(identities.len(), 1000);
    assert_eq!(
        identities
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        1000
    );
}

fn raw_publication_snapshot(
    frontier: crate::context::resources::LedgerFrontier,
) -> crate::model::RequestSnapshot {
    crate::model::RequestSnapshot::new(
        crate::model::RequestIdentity {
            attempt_id: attempt(),
            turn: TurnId::new("1"),
            retry_number: 0,
        },
        frontier,
        "system".into(),
        Vec::new(),
        crate::runtime::RuntimeResourceRevision::new(1),
        crate::model::invocation::ModelInvocationConfig {
            model: "scripted".into(),
            protocol: crate::model::ModelProtocol::OpenAiChatCompletions,
            max_output_tokens: 100,
            request_params: crate::model::invocation::RequestParams::new(),
            capabilities: crate::model::ModelCapabilities::text_only(true, true),
            compat: crate::model::ModelCompat::default(),
        },
        4096,
        None,
        false,
        Vec::new(),
        crate::runtime::identity::CapabilityRevision::new(1),
        crate::context::assembly::ContextGeneration {
            id: 1,
            contributors: Vec::new(),
        },
        None,
        Vec::new(),
    )
}

fn raw_publication_store(
    conversation_id: ConversationId,
) -> (
    crate::durable::SqliteConversationStore,
    crate::publication::PublicationStreamStart,
) {
    let store = crate::durable::SqliteConversationStore::in_memory(conversation_id).unwrap();
    let snapshot = raw_publication_snapshot(crate::context::resources::LedgerFrontier(
        store.observation_read_cut().unwrap().ledger_through,
    ));
    store
        .commit_model_turn_start(&[], &snapshot, chrono::Utc::now())
        .unwrap();
    let start = crate::publication::PublicationStreamStart {
        stream_id: crate::runtime::identity::PublicationStreamId::for_request(
            &snapshot.identity.attempt_id,
            &snapshot.provisional_message_id,
        ),
        attempt_id: snapshot.identity.attempt_id.clone(),
        turn_id: snapshot.identity.turn.clone(),
        request_id: snapshot.request_id.clone(),
        message_id: snapshot.provisional_message_id.clone(),
    };
    store.open_publication_stream(&start).unwrap();
    (store, start)
}

fn assert_raw_publication_range(
    store: &dyn crate::durable::ConversationStore,
    reference: &crate::observation::content::ContentRef,
    forged: &crate::observation::content::ContentRef,
    raw: &str,
) {
    use base64::Engine as _;
    let chunk = store.content_read(reference, 0, 65_536).unwrap();
    assert_eq!(chunk.readable_end, reference.readable_end);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&chunk.data_base64)
            .unwrap(),
        raw.as_bytes()
    );
    let rejected = store
        .content_read(forged, 0, 65_536)
        .expect_err("a forged frontier must return no bytes");
    assert!(
        matches!(
            rejected,
            crate::durable::ConversationStoreError::InvalidReference(_)
        ),
        "exact native source failure: {rejected:?}"
    );
    assert_eq!(
        crate::durable::SqliteConversationStore::content_read_work(),
        (0, 0),
        "frontier authorization rejects before any body range read"
    );
}

#[test]
fn t19_raw_assembled_arguments_and_observed_live_frontier_are_exact() {
    let mut projection = projection();
    let (store, start) = raw_publication_store(projection.snapshot.conversation_id.clone());
    let staged_frame = |sequence, payload| {
        let frame = PublicationFrame {
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            sequence,
            payload,
        };
        store
            .stage_publication_frames(std::slice::from_ref(&frame))
            .unwrap();
        frame
    };
    projection.apply(ConversationObservation::PublicationOpened {
        attempt_id: attempt(),
        start: start.clone(),
    });
    let call = ToolCall {
        id: ToolCallId::new("raw-call"),
        tool_id: ToolId::new("tool-alpha"),
        name: "alpha".into(),
        arguments: serde_json::json!({"n":1000.0}),
    };
    projection.apply(ConversationObservation::Publication {
        attempt_id: attempt(),
        frame: staged_frame(
            0,
            PublicationPayload::ProposedToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: ToolCallStart {
                    id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    name: call.name.clone(),
                },
            },
        ),
    });
    let raw = " { \"n\": 1e3 } ";
    let raw = raw.to_owned();
    projection.apply(ConversationObservation::Publication {
        attempt_id: attempt(),
        frame: staged_frame(
            1,
            PublicationPayload::ProposedToolCallArgumentsSuffix {
                block_index: ContentBlockIndex::new(0),
                call_id: call.id.clone(),
                suffix: raw.clone(),
            },
        ),
    });
    let RuntimeClientEvent::BlockContentAdvanced { content_ref, .. } =
        projection.replay.back().unwrap().1.facts().last().unwrap()
    else {
        panic!("bounded native advancement")
    };
    let reference = content_ref.clone();
    projection.validate_live_content_ref(&reference).unwrap();
    let mut forged = reference.clone();
    forged.readable_end += 1;
    // Scope validation does not grant bytes beyond the original native frontier.
    projection.validate_live_content_ref(&forged).unwrap();
    assert_raw_publication_range(&store, &reference, &forged, &raw);
    projection.apply(ConversationObservation::Publication {
        attempt_id: attempt(),
        frame: staged_frame(
            2,
            PublicationPayload::ProposedToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call,
            },
        ),
    });
    let RuntimeClientEvent::ToolCallAssembled { arguments_json, .. } =
        projection.replay.back().unwrap().1.facts().last().unwrap()
    else {
        panic!("assembly remains distinct")
    };
    assert_eq!(arguments_json, &raw);
}

/// T14/T20: Status carries its own native presentation, without examining
/// unrelated immutable Job body metadata held by the native registry snapshot.
#[test]
fn t14_t20_status_background_never_clones_or_serializes_hidden_job_body() {
    let result = std::sync::Arc::new(crate::scripted_suites::support::fake::success_result(
        &"hidden result ".repeat(700_000),
    ));
    let status = crate::context::AgentStatus {
        generated_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        sections: vec![crate::context::AgentStatusSection {
            id: crate::context::AgentStatusSectionId::new("background_execution"),
            data: crate::context::AgentStatusSectionData::BackgroundExecution {
                executions: vec![crate::tools::background::BackgroundExecutionSnapshot {
                    bash: None,
                    execution_id: crate::runtime::identity::ToolExecutionId::new(
                        "exec_215a03ee-2332-70b6-8e2d-634da8066f98",
                    ),
                    tool_id: std::sync::Arc::new(crate::runtime::identity::ToolId::new(
                        "private hidden identity ".repeat(100_000),
                    )),
                    tool_name: "visible native name".into(),
                    state: crate::tools::background::BackgroundLifecycle::PublishingTerminal,
                    progress: None,
                    result: Some(std::sync::Arc::clone(&result)),
                }],
                omitted_count: 73,
            },
        }],
    };
    let count = std::sync::Arc::strong_count(&result);
    let view = crate::observation::status::prepared_status(
        &attempt(),
        1,
        &MessageId::new("status-source"),
        &crate::context::ContributionOpportunities::default(),
        None,
        &status,
        None,
    );
    assert_eq!(std::sync::Arc::strong_count(&result), count);
    let encoded = serde_json::to_vec(&view).unwrap();
    assert!(encoded.len() < 4096);
    assert!(
        !String::from_utf8(encoded)
            .unwrap()
            .contains("private hidden")
    );
    let crate::observation::status::RuntimeClientStatusSection::BackgroundExecutions {
        executions,
        omitted_count,
    } = &view.sections[0]
    else {
        panic!("native Background section")
    };
    assert_eq!(*omitted_count, 73);
    assert_eq!(executions.len(), 1);
    assert_eq!(
        executions[0].state,
        crate::tools::background::BackgroundLifecycle::PublishingTerminal
    );
    assert!(view.rendered.contains("publishing_terminal"));
}
