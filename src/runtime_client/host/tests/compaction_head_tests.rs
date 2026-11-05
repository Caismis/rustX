// A committed compaction owns its head before asynchronous domain refinement.
use super::*;
use crate::durable::{JournalStore, ObservationStore, ProjectionStore};
use crate::runtime_client::types::RequestId;

#[test]
#[allow(clippy::too_many_lines)] // One bounded persisted operation covers cold start, accepted progress and cancellation.
fn cold_read_restores_full_bounded_progress_without_trace_or_completion() {
    use crate::durable::{AuxiliaryStore, CanonicalStore, SqliteConversationStore};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEventEnvelope};
    use crate::model::auxiliary::{
        AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryCallStart,
        AuxiliaryFingerprint, AuxiliaryFinishReason, AuxiliaryInputEvidence,
        AuxiliaryObservedProviderTerminal, AuxiliaryRequestEvidence, AuxiliaryRequestStart,
        AuxiliarySourceRange, AuxiliarySourceRole, AuxiliaryUsageCompleteness,
    };
    use crate::runtime::identity::{CompactionOperationId, ConversationId, EventId};
    let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
    store.initialize(&[]).unwrap();
    let operation_id = CompactionOperationId::new("cold-progress");
    let append = |event| {
        store
            .append_event(RuntimeEventEnvelope {
                schema_version: EVENT_SCHEMA_VERSION,
                event_id: EventId::new(""),
                sequence: 0,
                conversation_id: store.conversation_id().clone(),
                attempt_id: None,
                turn_id: None,
                timestamp: chrono::Utc::now(),
                tool_occurrence: None,
                observation: None,
                event,
            })
            .unwrap()
    };
    append(RuntimeEvent::CompactionStarted {
        operation_id: operation_id.clone(),
    });
    let started_cut = store.observation_read_cut().unwrap();
    let started = super::super::durable_bootstrap::read(&store, &started_cut).unwrap();
    assert!(!started.context.compaction_in_progress);
    assert_eq!(
        started
            .context
            .compaction_progress
            .unwrap()
            .accepted_fragments,
        0
    );
    let ranges: Vec<_> = (0..32)
        .map(|projection_index| AuxiliarySourceRange {
            projection_index,
            role: AuxiliarySourceRole::User,
            byte_start: 0,
            byte_end: 1,
        })
        .collect();
    let call_id = crate::runtime::identity::AuxiliaryCallId::generate();
    let request_id = crate::runtime::identity::RequestId::new("auxiliary:cold-progress");
    store
        .commit_auxiliary_call_start(
            &AuxiliaryCallStart {
                call_id: call_id.clone(),
                purpose: AuxiliaryAttribution::new("summary").unwrap(),
                trigger: AuxiliaryAttribution::new("manual").unwrap(),
                business_source: AuxiliaryBusinessSource::Compaction {
                    operation_id: operation_id.clone(),
                    preceding_call: None,
                },
                input: AuxiliaryInputEvidence::Compaction {
                    cut: store.compaction_resource_cut().unwrap(),
                    source_ranges: ranges.clone(),
                    previous_summary: None,
                    output_strategy: crate::model::structured::StructuredOutputStrategy::Prompt,
                },
                semantic_input_fingerprint: AuxiliaryFingerprint::new("a".repeat(64)).unwrap(),
                invocation_fingerprint: AuxiliaryFingerprint::new("b".repeat(64)).unwrap(),
                selected_message_ids: vec![],
            },
            chrono::Utc::now(),
        )
        .unwrap();
    store
        .commit_auxiliary_request_start(
            &AuxiliaryRequestStart {
                call_id: call_id.clone(),
                ordinal: 0,
                request_id: request_id.clone(),
                model: "fixture/summary".into(),
            },
            chrono::Utc::now(),
        )
        .unwrap();
    let terminal = crate::durable::auxiliary::auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        chrono::Utc::now(),
        RuntimeEvent::ModelRequestCompleted {
            owner: crate::model::ModelRequestOwner::Auxiliary {
                call_id: call_id.clone(),
                ordinal: 0,
            },
            request_id: request_id.clone(),
            finish_reason: crate::model::ModelFinishReason::Stop,
            usage: None,
            generation: None,
        },
    )
    .unwrap();
    let settled = crate::durable::auxiliary::auxiliary_settlement_envelope(
        store.conversation_id().clone(),
        chrono::Utc::now(),
        RuntimeEvent::AuxiliaryCallSettled {
            call_id: call_id.clone(),
            outcome: AuxiliaryCallOutcome::Completed,
            request_evidence: Some(AuxiliaryRequestEvidence {
                request_id,
                observed_provider_terminal: Some(AuxiliaryObservedProviderTerminal::Completed {
                    finish_reason: AuxiliaryFinishReason::Stop,
                }),
                usage: None,
                usage_completeness: AuxiliaryUsageCompleteness::Unreported,
                remote_execution_uncertainty: false,
            }),
        },
    )
    .unwrap();
    store
        .commit_auxiliary_request_settlement(terminal, settled)
        .unwrap();
    let generation = crate::context::checkpoint::SummaryGenerationRef {
        conversation_id: store.conversation_id().clone(),
        call_id,
        through_journal: store.auxiliary_audit_cut().unwrap().journal,
    };
    let event = RuntimeEvent::CompactionChunkAccepted {
        operation_id: operation_id.clone(),
        accepted_fragments: 1,
        source_ranges: ranges.clone(),
        generation: Some(generation),
    };
    assert!(!crate::runtime::observation::trace_fact_requires_publication(&event));
    append(event);
    let cut = store.observation_read_cut().unwrap();
    let cold = super::super::durable_bootstrap::read(&store, &cut).unwrap();
    assert!(!cold.context.compaction_in_progress);
    let progress = cold.context.compaction_progress.unwrap();
    assert_eq!(progress.operation_id, operation_id);
    assert_eq!(progress.accepted_fragments, 1);
    assert_eq!(progress.source_ranges, ranges);
    assert_eq!(cold.context.compaction_count, 0);
    append(RuntimeEvent::CompactionCancelled { operation_id });
    let cut = store.observation_read_cut().unwrap();
    let cold = super::super::durable_bootstrap::read(&store, &cut).unwrap();
    assert!(!cold.context.compaction_in_progress);
    assert!(cold.context.compaction_progress.is_none());
    assert!(cold.context.compaction_error.is_none());
    assert_eq!(cold.context.compaction_count, 0);
}

fn committed_view(
    outcome: &crate::runtime::conversation_runtime::ManualCompactionOutcome,
) -> crate::observation::head::RuntimeClientCompactionView {
    let crate::runtime::conversation_runtime::ManualCompactionOutcome::Compacted {
        generation,
        checkpoint_id,
        activation_id,
        tokens_before,
        estimated_tokens_after,
    } = outcome
    else {
        panic!("expected committed compaction");
    };
    crate::observation::head::RuntimeClientCompactionView {
        generation: *generation,
        checkpoint_id: checkpoint_id.clone(),
        activation_id: *activation_id,
        tokens_before: *tokens_before,
        estimated_tokens_after: *estimated_tokens_after,
    }
}

struct ParkedProjection<'a>(&'a super::super::ClientInner);
impl Drop for ParkedProjection<'_> {
    fn drop(&mut self) {
        self.0.unpark_projection_worker();
    }
}

async fn completed_turn(fixture: &HostFixture, subscription: &EventSubscription) {
    fixture
        .runtime
        .submit_inbound(submit_content("original turn before compaction"))
        .unwrap();
    receive_until(subscription, |event| {
        matches!(event.event, RuntimeClientEvent::AttemptSettled { .. })
    })
    .await;
    tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
        loop {
            let released = fixture.runtime.settlement_signal().notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if !fixture.runtime.has_current_attempt() {
                break;
            }
            released.await;
        }
    })
    .await
    .expect("native physical attempt owner releases");
    fixture.host.projection_settled().await;
}

fn assert_compaction_head(
    context: &crate::runtime_client::snapshot::RuntimeClientContextView,
    outcome: &crate::runtime::conversation_runtime::ManualCompactionOutcome,
) {
    let outcome = committed_view(outcome);
    assert_eq!(context.compaction_count, outcome.generation);
    let compaction = context.latest_compaction.as_ref().unwrap();
    assert_eq!(compaction.generation, outcome.generation);
    assert_eq!(compaction.activation_id, outcome.activation_id);
    assert_eq!(compaction.checkpoint_id, outcome.checkpoint_id);
    let head = context.projection_head.as_ref().unwrap();
    assert_eq!(head.activation_id, outcome.activation_id);
    assert_eq!(head.checkpoint_id, outcome.checkpoint_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_compaction_head_is_atomic_with_context_while_domain_reads_are_parked() {
    let (model, fixture) = host_fixture(
        (0..4).map(|_| one_turn_stop()).collect(),
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (attachment, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let initial = fixture.host.checkpoint().unwrap();
    let subscription = attachment.subscribe_events(initial.cursor).unwrap();
    completed_turn(&fixture, &subscription).await;
    let first = fixture.runtime.compact_context(None, None).await.unwrap();
    fixture.host.projection_settled().await;
    assert_compaction_head(&fixture.host.checkpoint().unwrap().snapshot.context, &first);
    completed_turn(&fixture, &subscription).await;
    let before = fixture.host.checkpoint().unwrap();
    assert_compaction_head(&before.snapshot.context, &first);

    fixture.host.inner.park_projection_worker();
    let parked = ParkedProjection(&fixture.host.inner);
    let subscription = attachment.subscribe_events(before.cursor).unwrap();
    let second = fixture.runtime.compact_context(None, None).await.unwrap();
    let after = fixture.host.checkpoint().unwrap();
    assert_eq!(committed_view(&second).generation, 2);
    assert_compaction_head(&after.snapshot.context, &second);
    let store = fixture.store.as_ref().unwrap();
    let native = store.observation_read_cut().unwrap();
    assert_eq!(after.read_cut.source_cut.as_ref(), Some(&native));
    assert_eq!(after.read_cut.through, native.read.journal);
    assert_eq!(
        after.snapshot.context.projection_head,
        store
            .observation_projection_head(native.read.journal)
            .unwrap()
    );
    assert_compacted_replay(&subscription, before.cursor, &after, &second).await;
    let original = store.read_events(None, 128).unwrap().events;
    let completions: Vec<_> = original
        .iter()
        .filter(|event| matches!(event.event, RuntimeEvent::CompactionCompleted { .. }))
        .collect();
    assert_eq!(completions.len(), 2);
    assert_eq!(completions[1].sequence, native.read.journal);
    assert_eq!(model.requests().len(), 4);
    drop(parked);
    fixture.host.projection_settled().await;
    assert_compaction_head(
        &fixture.host.checkpoint().unwrap().snapshot.context,
        &second,
    );
    fixture.runtime.shutdown().await.unwrap();
    fixture.host.inner.drain_projection().await.unwrap();
}

async fn assert_compacted_replay(
    subscription: &EventSubscription,
    before: RuntimeClientCursor,
    after: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    outcome: &crate::runtime::conversation_runtime::ManualCompactionOutcome,
) {
    let mut cursor = before;
    let mut completed = 0;
    while cursor < after.cursor {
        let EventDelivery::Event(record) = subscription.next().await else {
            panic!("the original complete transition remains replayable");
        };
        assert_eq!(record.cursor.get(), cursor.get() + 1);
        cursor = record.cursor;
        let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &record.event else {
            panic!("one complete publication record");
        };
        assert_eq!(metadata.read_cut.revision, record.cursor.get());
        for fact in record.event.facts() {
            if let RuntimeClientEvent::ContextCompacted { context, .. } = fact {
                completed += 1;
                assert_compaction_head(context, outcome);
                assert_eq!(metadata.read_cut.through, after.read_cut.through);
                assert_eq!(metadata.read_cut.source_cut, after.read_cut.source_cut);
            }
        }
    }
    assert_eq!(cursor, after.cursor);
    assert_eq!(completed, 1);
}

fn pause_compaction_response(
    host: &RuntimeClientHost,
) -> (
    tokio::sync::oneshot::Receiver<crate::runtime::conversation_runtime::ManualCompactionOutcome>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (committed, receipt) = tokio::sync::oneshot::channel();
    let (release, held) = tokio::sync::oneshot::channel();
    assert!(
        host.inner
            .manual_compaction_response_gate
            .lock()
            .unwrap()
            .replace((committed, held))
            .is_none()
    );
    (receipt, release)
}

fn assert_receipt(
    result: RuntimeClientResult,
    outcome: &crate::runtime::conversation_runtime::ManualCompactionOutcome,
) {
    let outcome = committed_view(outcome);
    let RuntimeClientResult::ContextCompacted {
        compaction: Some(receipt),
    } = result
    else {
        panic!("success must retain this invocation's exact native receipt");
    };
    assert_eq!(receipt.generation, outcome.generation);
    assert_eq!(receipt.checkpoint_id, outcome.checkpoint_id);
    assert_eq!(receipt.activation_id, outcome.activation_id);
    assert_eq!(receipt.tokens_before, outcome.tokens_before);
    assert_eq!(
        receipt.estimated_tokens_after,
        outcome.estimated_tokens_after
    );
}

fn original_compactions(
    fixture: &HostFixture,
) -> Vec<crate::runtime::conversation_runtime::ManualCompactionOutcome> {
    fixture
        .store
        .as_ref()
        .unwrap()
        .read_events(None, 128)
        .unwrap()
        .events
        .into_iter()
        .filter_map(|envelope| match envelope.event {
            RuntimeEvent::CompactionCompleted {
                generation,
                checkpoint_id,
                activation_id,
                tokens_before,
                estimated_tokens_after,
                ..
            } => Some(
                crate::runtime::conversation_runtime::ManualCompactionOutcome::Compacted {
                    generation,
                    checkpoint_id,
                    activation_id,
                    tokens_before,
                    estimated_tokens_after,
                },
            ),
            _ => None,
        })
        .collect()
}

struct ProjectionHeaderFault(rusqlite::Connection);
impl ProjectionHeaderFault {
    fn new(fixture: &HostFixture) -> Self {
        let connection =
            rusqlite::Connection::open(fixture.directory.path().join("conversation.sqlite"))
                .unwrap();
        connection.execute_batch("CREATE TRIGGER refuse_receipt_projection_header BEFORE UPDATE OF observation_header ON projection_activations BEGIN SELECT RAISE(FAIL,'optional receipt projection header'); END;").unwrap();
        Self(connection)
    }
}
impl Drop for ProjectionHeaderFault {
    fn drop(&mut self) {
        self.0
            .execute_batch("DROP TRIGGER refuse_receipt_projection_header")
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_receipt_survives_unavailable_published_context() {
    let (model, fixture) = host_fixture(
        vec![one_turn_stop(), one_turn_stop()],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (observer, _) = fixture
        .host
        .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let before = fixture.host.checkpoint().unwrap();
    let subscription = observer.subscribe_events(before.cursor).unwrap();
    completed_turn(&fixture, &subscription).await;
    let published = fixture.host.checkpoint().unwrap().snapshot.context;
    assert!(published.latest_compaction.is_none());
    let fault = ProjectionHeaderFault::new(&fixture);
    let result = fixture.host.compact_context(None, None).await.unwrap();
    let original = original_compactions(&fixture);
    assert_eq!(original.len(), 1);
    assert_receipt(result, &original[0]);
    // The optional derived header failed inside its own savepoint. Native J
    // and the immutable checkpoint committed; observation remains unavailable.
    let native_head = fixture
        .store
        .as_ref()
        .unwrap()
        .projection_head()
        .unwrap()
        .unwrap();
    let original_view = committed_view(&original[0]);
    assert_eq!(
        native_head.activation.activation,
        original_view.activation_id
    );
    assert_eq!(
        native_head.activation.checkpoint_id,
        original_view.checkpoint_id
    );
    let retained = fixture
        .host
        .inner
        .lock_published_state()
        .projection
        .snapshot_ref()
        .context
        .latest_compaction
        .clone();
    assert_eq!(retained, published.latest_compaction);
    assert!(matches!(
        fixture.host.checkpoint(),
        Err(RuntimeClientError::ObservationUnavailable { .. })
    ));
    assert_eq!(model.requests().len(), 2);
    assert_eq!(original_compactions(&fixture), original);
    drop(fault);
    fixture.runtime.shutdown().await.unwrap();
    fixture.host.inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_wire_receipt_keeps_a_after_a_valid_successor_b_commits() {
    let (model, fixture) = host_fixture(
        (0..4).map(|_| one_turn_stop()).collect(),
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (observer, _) = fixture
        .host
        .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let initial = fixture.host.checkpoint().unwrap();
    let subscription = observer.subscribe_events(initial.cursor).unwrap();
    completed_turn(&fixture, &subscription).await;
    let endpoint = Arc::new(crate::runtime_client::endpoint::RuntimeClientEndpoint::new(
        &fixture.host,
    ));
    assert!(
        endpoint
            .handle_request(RuntimeClientRequest::Initialize {
                id: RequestId::new(40),
                protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
            })
            .error
            .is_none()
    );
    let (committed, release) = pause_compaction_response(&fixture.host);
    let first_endpoint = endpoint.clone();
    let first = tokio::spawn(async move {
        first_endpoint
            .handle_request_async(RuntimeClientRequest::CompactContext {
                id: RequestId::new(41),
                instructions: None,
            })
            .await
    });
    let a = committed.await.unwrap();
    assert_eq!(committed_view(&a).generation, 1);
    assert!(!first.is_finished());
    // Native A already released maintenance. A genuine new Attempt supplies
    // new history before B; neither control nor native ownership is bypassed.
    completed_turn(&fixture, &subscription).await;
    let b_response = endpoint
        .handle_request_async(RuntimeClientRequest::CompactContext {
            id: RequestId::new(42),
            instructions: None,
        })
        .await;
    assert_eq!(b_response.id, RequestId::new(42));
    assert!(b_response.error.is_none());
    let original = original_compactions(&fixture);
    assert_eq!(original.len(), 2);
    assert_eq!(original[0], a);
    let a_view = committed_view(&a);
    let b_view = committed_view(&original[1]);
    assert_eq!(b_view.generation, 2);
    assert_ne!(b_view.checkpoint_id, a_view.checkpoint_id);
    assert_ne!(b_view.activation_id, a_view.activation_id);
    assert_receipt(b_response.result.unwrap(), &original[1]);
    fixture.host.projection_settled().await;
    assert_compaction_head(
        &fixture.host.checkpoint().unwrap().snapshot.context,
        &original[1],
    );
    release.send(()).unwrap();
    let a_response = first.await.unwrap();
    // Exercise the actual endpoint response and JSONL DTO, including its
    // original request ID. Reading latest Projection here would return B.
    let wire = serde_json::to_vec(&a_response).unwrap();
    let decoded: crate::runtime_client::types::RuntimeClientResponse =
        serde_json::from_slice(&wire).unwrap();
    assert_eq!(decoded.id, RequestId::new(41));
    assert!(decoded.error.is_none());
    assert_receipt(decoded.result.unwrap(), &a);
    assert_eq!(model.requests().len(), 4);
    assert_eq!(original_compactions(&fixture), original);
    fixture.runtime.shutdown().await.unwrap();
    fixture.host.inner.drain_projection().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_lost_stdio_response_reconciles_without_replaying_operation() {
    use crate::runtime_client::transport::stdio::{StdioSessionEnd, serve_stdio_jsonl_with_io};
    use tokio::io::AsyncWriteExt as _;

    let (model, fixture) = host_fixture(
        vec![one_turn_stop(), one_turn_stop()],
        ToolRegistry::new(),
        status_engine(),
    )
    .await;
    let (observer, _) = fixture
        .host
        .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    let before = fixture.host.checkpoint().unwrap();
    let subscription = observer.subscribe_events(before.cursor).unwrap();
    completed_turn(&fixture, &subscription).await;
    let endpoint = crate::runtime_client::endpoint::RuntimeClientEndpoint::new(&fixture.host);
    assert!(
        endpoint
            .handle_request(RuntimeClientRequest::Initialize {
                id: RequestId::new(50),
                protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
            })
            .error
            .is_none()
    );
    let (committed, release) = pause_compaction_response(&fixture.host);
    let (mut client, server) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(server);
    let transport = tokio::spawn(serve_stdio_jsonl_with_io(endpoint, reader, writer));
    let mut request = serde_json::to_vec(&RuntimeClientRequest::CompactContext {
        id: RequestId::new(51),
        instructions: None,
    })
    .unwrap();
    request.push(b'\n');
    client.write_all(&request).await.unwrap();
    let outcome = committed.await.unwrap();
    let original = original_compactions(&fixture);
    assert_eq!(original, vec![outcome.clone()]);
    drop(client);
    release.send(()).unwrap();
    assert_eq!(
        transport.await.unwrap().unwrap(),
        StdioSessionEnd::OutputBrokenPipe
    );
    // A new attachment reconciles the committed native operation through the
    // existing checkpoint; it never submits another CompactContext request.
    let (reconnected, _) = fixture
        .host
        .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .unwrap();
    fixture.host.projection_settled().await;
    assert_compaction_head(
        &fixture.host.checkpoint().unwrap().snapshot.context,
        &outcome,
    );
    assert_eq!(original_compactions(&fixture), original);
    assert_eq!(model.requests().len(), 2);
    reconnected.detach();
    fixture.runtime.shutdown().await.unwrap();
    fixture.host.inner.drain_projection().await.unwrap();
}
