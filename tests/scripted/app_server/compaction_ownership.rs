//! Manager lease transfer is independent of the native compaction reply.
use super::*;

/// The real terminal transaction commits before its acknowledgement is lost;
/// exact confirmation then fails. The executor must join both native threads.
async fn fail_summary_confirmation(
    f: &Fixture,
    live: &ConversationRuntime,
    gates: crate::runtime::auxiliary::executor::AuxiliaryWriteTestGates,
) {
    f.gates[1].release();
    gates.committed.await.unwrap();
    drop(gates.release_ack);
    gates.confirming.await.unwrap();
    drop(gates.release_confirmation);
    live.join_auxiliary_for_test().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proven_auxiliary_exit_releases_failed_unload_for_exclusive_recovery() {
    bounded(async {
        let f = Fixture::with_summary_gate().await;
        let _headers = ReleaseHeaders(f.gates.clone());
        f.gates[0].release();
        let loaded = f.load(0).await.unwrap().unwrap();
        let id = loaded.conversation_id().clone();
        let incarnation = loaded.incarnation_id();
        let live = loaded.inspect_runtime().unwrap();
        complete_history_turn(&live).await;
        let compaction = tokio::spawn({
            let live = live.clone();
            async move { live.compact_context(None).await }
        });
        f.gates[1].wait_entered().await;
        let gates = live.hold_auxiliary_settlement_for_test();
        fail_summary_confirmation(&f, &live, gates).await;
        assert!(
            compaction.await.unwrap().is_err(),
            "uncertainty is not success"
        );
        assert!(live.durability_failure().is_some());
        let original = journal(&live);
        assert_one_summary_chain(&original, true, false);
        assert!(
            !original
                .iter()
                .any(|fact| matches!(fact.event, RuntimeEvent::CompactionCompleted { .. }))
        );
        let weak = live.weak_inner();
        drop(live);
        assert!(
            f.manager.unload(&id).await.is_err(),
            "preserve logical failure"
        );
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        assert!(
            weak.upgrade().is_none(),
            "native and projection owners exited"
        );
        let acquisitions = f.manager.probe(&id).acquisitions.load(Ordering::SeqCst);
        let requests = f.provider.request_bodies().len();
        let recovered = f.load(0).await.unwrap().unwrap();
        assert_ne!(recovered.incarnation_id(), incarnation);
        assert_eq!(
            f.manager.probe(&id).acquisitions.load(Ordering::SeqCst),
            acquisitions + 1
        );
        let reopened = recovered.inspect_runtime().unwrap();
        assert_eq!(
            journal(&reopened),
            original,
            "exclusive recovery never redispatches the old RequestId"
        );
        assert_eq!(f.provider.request_bodies().len(), requests);
        assert!(
            reopened
                .tool_runtime()
                .durable_store()
                .projection_head()
                .unwrap()
                .is_none()
        );
        drop(reopened);
        f.close().await;
    })
    .await;
}

async fn auxiliary_failure_orders_coordinator_mutation(
    model_selection: bool,
    mutation_first: bool,
) {
    let f = Fixture::with_summary_gate().await;
    let _headers = ReleaseHeaders(f.gates.clone());
    f.gates[0].release();
    let loaded = f.load(0).await.unwrap().unwrap();
    let id = loaded.conversation_id().clone();
    let live = loaded.inspect_runtime().unwrap();
    complete_history_turn(&live).await;
    let compaction = tokio::spawn({
        let live = live.clone();
        async move { live.compact_context(None).await }
    });
    f.gates[1].wait_entered().await;
    let writer = live.hold_auxiliary_settlement_for_test();
    let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
    let release = mutation_first.then(|| gate.arm_scoped());
    live.install_durability_mutation_probe(
        (!model_selection).then(|| gate.clone()),
        model_selection.then(|| gate.clone()),
    );
    let mut selection = live.model_config();
    selection.max_output_tokens = Some(17);
    let start_mutation = || {
        if model_selection {
            let manager = f.manager.clone();
            let session = f.sessions[0].id.clone();
            let selection = selection.clone();
            tokio::spawn(async move {
                manager
                    .set_model(&session, selection)
                    .await
                    .map(|_| ())
                    .map_err(|e| format!("{e:?}"))
            })
        } else {
            let live = live.clone();
            tokio::task::spawn_blocking(move || {
                live.submit_inbound(input("ordered inbound"))
                    .map(|_| ())
                    .map_err(|e| format!("{e:?}"))
            })
        }
    };
    let mutation = mutation_first.then(start_mutation);
    if mutation_first {
        let gate = gate.clone();
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
    }
    fail_summary_confirmation(&f, &live, writer).await;
    if mutation_first {
        assert!(
            live.durability_failure().is_none(),
            "the global reaper must finish without publishing ahead of durable acceptance"
        );
        drop(release);
    } else {
        assert!(live.durability_failure().is_some());
    }
    let mutation = mutation.unwrap_or_else(start_mutation).await.unwrap();
    assert_eq!(mutation.is_ok(), mutation_first, "{mutation:?}");
    assert!(live.durability_failure().is_some());
    assert!(compaction.await.unwrap().is_err());
    if model_selection {
        let catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&f.sessions[0].id, None).unwrap();
        assert_eq!(
            settings
                .model
                .and_then(|selection| selection.max_output_tokens),
            mutation_first.then_some(17)
        );
    } else {
        assert_eq!(
            live.tool_runtime()
                .durable_store()
                .load_pending()
                .unwrap()
                .len(),
            usize::from(mutation_first)
        );
    }
    drop(live);
    assert!(f.manager.unload(&id).await.is_err());
    assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auxiliary_failure_and_inbound_acceptance_have_one_commit_order() {
    bounded(async {
        for mutation_first in [true, false] {
            auxiliary_failure_orders_coordinator_mutation(false, mutation_first).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auxiliary_failure_and_model_selection_have_one_commit_order() {
    bounded(async {
        for mutation_first in [true, false] {
            auxiliary_failure_orders_coordinator_mutation(true, mutation_first).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_compaction_transfer_releases_its_lease_before_reply_or_waiter_loss() {
    bounded(async {
        for lose_reply in [false, true] {
            let f = Fixture::with_session_count(None, 1).await;
            let loaded = f.load(0).await.unwrap().unwrap();
            let id = loaded.conversation_id().clone();
            let incarnation = loaded.incarnation_id();
            let client = loaded.client();
            let (attachment, _pin) = client.attach().unwrap();
            let authority = attachment.operation_authority().unwrap();
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
            let mut reply = Some(
                client
                    .start_compaction_operation(move |_| async move {
                        // Real native admission owns the business task before the
                        // manager releases its lease. Only reply consumption is gated.
                        let native = authority.admit_compaction_transfer(None)?;
                        let tail: CompactionTail = Box::pin(async move {
                            let _ = entered_tx.send(());
                            release_rx.await.unwrap();
                            let result = native.await;
                            let _ = finished_tx.send(());
                            result
                        });
                        Ok(tail)
                    })
                    .unwrap(),
            );
            entered_rx.await.unwrap();
            assert!(matches!(
                reply.as_mut().unwrap().try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            let diagnostic = f
                .manager
                .diagnostics()
                .sessions
                .into_iter()
                .find(|session| session.conversation_id == id)
                .unwrap();
            assert_eq!(diagnostic.operations, 0, "reply wait owns no manager lease");
            if lose_reply {
                drop(reply.take());
            }

            // An explicit unload must reach native shutdown and release the
            // actual writer allocation while the response is still parked.
            f.manager.unload(&id).await.unwrap();
            assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
            let successor = f.load(0).await.unwrap().unwrap();
            assert_ne!(successor.incarnation_id(), incarnation);
            release_tx.send(()).unwrap();
            finished_rx.await.unwrap();
            if let Some(reply) = reply {
                assert!(
                    reply.await.unwrap().is_err(),
                    "empty history cannot compact"
                );
            }
            assert!(f.provider.request_bodies().is_empty());
            f.close().await;
        }
    })
    .await;
}

// Header gates are asynchronous, but still release them on an assertion failure
// so a failed scenario cannot leave a provider handler parked at teardown.
struct ReleaseHeaders([Arc<HeaderGate>; 2]);
impl Drop for ReleaseHeaders {
    fn drop(&mut self) {
        for gate in &self.0 {
            gate.release();
        }
    }
}

struct ParkedCompactionReply {
    reply: tokio::sync::oneshot::Receiver<CompactionReply>,
    admitted: tokio::sync::oneshot::Receiver<()>,
    completed: tokio::sync::oneshot::Receiver<CompactionReply>,
    release: tokio::sync::oneshot::Sender<()>,
}

fn park_transferred_reply(
    client: &ManagedRuntimeClient,
    attachment: &crate::runtime_client::attachment::RuntimeAttachment,
) -> ParkedCompactionReply {
    let authority = attachment.operation_authority().unwrap();
    let (admitted_tx, admitted) = tokio::sync::oneshot::channel();
    let (completed_tx, completed) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let reply = client
        .start_compaction_operation(move |_| async move {
            let native = authority.admit_compaction_transfer(None)?;
            let tail: CompactionTail = Box::pin(async move {
                let _ = admitted_tx.send(());
                let result = native.await;
                let _ = completed_tx.send(result.clone());
                // Only delivery is parked. Dropping this sender also releases
                // the gate, including when the enclosing scenario fails.
                let _ = released.await;
                result
            });
            Ok(tail)
        })
        .unwrap();
    ParkedCompactionReply {
        reply,
        admitted,
        completed,
        release,
    }
}

async fn complete_history_turn(runtime: &ConversationRuntime) {
    let settled = runtime.settlement_signal();
    let notification = settled.notified();
    tokio::pin!(notification);
    notification.as_mut().enable();
    // Exceed the default recent-history budget. A successor turn can then
    // retire the previous retained tail instead of replacing only the same
    // tiny summary with the fixture's identical response and making no progress.
    runtime
        .submit_inbound(input(&"important history for a real Summary ".repeat(1024)))
        .unwrap();
    notification.await;
    loop {
        let released = settled.notified();
        tokio::pin!(released);
        released.as_mut().enable();
        if !runtime.has_current_attempt() {
            break;
        }
        released.await;
    }
}

fn journal(runtime: &ConversationRuntime) -> Vec<crate::events::types::RuntimeEventEnvelope> {
    runtime
        .tool_runtime()
        .durable_store()
        .read_events(None, 256)
        .unwrap()
        .events
}

fn compaction_receipt(
    result: CompactionReply,
) -> crate::observation::head::RuntimeClientCompactionView {
    let crate::runtime_client::types::RuntimeClientResult::ContextCompacted {
        compaction: Some(receipt),
    } = result.unwrap()
    else {
        panic!("native success retains the invocation's complete receipt");
    };
    receipt
}

fn assert_compaction_snapshot(
    client: &ManagedRuntimeClient,
    expected: Option<&crate::observation::head::RuntimeClientCompactionView>,
    count: u64,
) {
    let context = client.checkpoint().unwrap().snapshot.context;
    assert_eq!(context.latest_compaction.as_ref(), expected);
    assert_eq!(context.compaction_count, count);
    assert!(!context.compaction_in_progress);
}

/// Assert the composed ownership chain, not just counts: one operation owns one
/// Call, which owns ordinal-zero Request, then exactly one terminal for each.
fn assert_one_summary_chain(
    journal: &[crate::events::types::RuntimeEventEnvelope],
    completed: bool,
    checkpoint_committed: bool,
) -> crate::runtime::identity::CompactionOperationId {
    use crate::model::auxiliary::{
        AuxiliaryBusinessSource, AuxiliaryCallOutcome, AuxiliaryUsageCompleteness,
    };
    use crate::model::snapshot::ModelRequestOwner;

    let facts: Vec<_> = journal
        .iter()
        .filter(|envelope| {
            matches!(
                &envelope.event,
                RuntimeEvent::CompactionStarted { .. }
                    | RuntimeEvent::CompactionCompleted { .. }
                    | RuntimeEvent::CompactionFailed { .. }
                    | RuntimeEvent::AuxiliaryCallStarted { .. }
                    | RuntimeEvent::AuxiliaryCallSettled { .. }
                    | RuntimeEvent::ModelRequestStarted {
                        owner: ModelRequestOwner::Auxiliary { .. },
                        ..
                    }
                    | RuntimeEvent::ModelRequestCompleted {
                        owner: ModelRequestOwner::Auxiliary { .. },
                        ..
                    }
                    | RuntimeEvent::ModelRequestFailed {
                        owner: ModelRequestOwner::Auxiliary { .. },
                        ..
                    }
            )
        })
        .collect();
    assert_eq!(
        facts.len(),
        6,
        "exactly one physical Summary and business terminal"
    );
    assert!(
        facts
            .iter()
            .all(|fact| fact.attempt_id.is_none() && fact.turn_id.is_none())
    );
    let RuntimeEvent::CompactionStarted { operation_id } = &facts[0].event else {
        panic!("operation starts before its child");
    };
    let RuntimeEvent::AuxiliaryCallStarted {
        call_id,
        business_source,
        ..
    } = &facts[1].event
    else {
        panic!("one owned Summary Call");
    };
    assert_eq!(
        business_source,
        &AuxiliaryBusinessSource::Compaction {
            operation_id: operation_id.clone()
        }
    );
    let RuntimeEvent::ModelRequestStarted {
        owner, request_id, ..
    } = &facts[2].event
    else {
        panic!("one physical Summary Request");
    };
    assert_eq!(
        owner,
        &ModelRequestOwner::Auxiliary {
            call_id: call_id.clone(),
            ordinal: 0
        }
    );
    match &facts[3].event {
        RuntimeEvent::ModelRequestCompleted {
            owner: terminal_owner,
            request_id: terminal_id,
            usage,
            ..
        } if completed => {
            assert_eq!(terminal_owner, owner);
            assert_eq!(terminal_id, request_id);
            assert!(usage.is_none());
        }
        RuntimeEvent::ModelRequestFailed {
            owner: terminal_owner,
            request_id: terminal_id,
            error,
            usage,
            ..
        } if !completed => {
            assert_eq!(terminal_owner, owner);
            assert_eq!(terminal_id, request_id);
            assert_eq!(error.kind, crate::model::error::ModelErrorKind::Cancelled);
            assert!(usage.is_none());
        }
        event => panic!("wrong physical terminal: {event:?}"),
    }
    let RuntimeEvent::AuxiliaryCallSettled {
        call_id: terminal_call,
        outcome,
        request_evidence: Some(evidence),
    } = &facts[4].event
    else {
        panic!("the Call settles atomically with its Request");
    };
    assert_eq!(terminal_call, call_id);
    assert_eq!(
        *outcome,
        if completed {
            AuxiliaryCallOutcome::Completed
        } else {
            AuxiliaryCallOutcome::Cancelled
        }
    );
    assert_eq!(&evidence.request_id, request_id);
    assert_eq!(
        evidence.usage_completeness,
        AuxiliaryUsageCompleteness::Unreported
    );
    assert!(
        evidence.usage.is_none(),
        "missing provider usage must not become zero"
    );
    assert_eq!(evidence.observed_provider_terminal.is_some(), completed);
    assert_eq!(evidence.remote_execution_uncertainty, !completed);
    assert_eq!(facts[4].sequence, facts[3].sequence + 1);
    match &facts[5].event {
        RuntimeEvent::CompactionCompleted {
            operation_id: terminal_operation,
            ..
        } if checkpoint_committed => assert_eq!(terminal_operation, operation_id),
        RuntimeEvent::CompactionFailed {
            operation_id: terminal_operation,
            ..
        } if !checkpoint_committed => assert_eq!(terminal_operation, operation_id),
        event => panic!("wrong business terminal: {event:?}"),
    }
    operation_id.clone()
}

fn assert_no_manager_operation(fixture: &Fixture, id: &ConversationId) {
    let diagnostic = fixture
        .manager
        .diagnostics()
        .sessions
        .into_iter()
        .find(|session| &session.conversation_id == id)
        .unwrap();
    assert_eq!(
        diagnostic.operations, 0,
        "native work and its parked reply own no manager lease"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_summary_reply_loss_and_cold_successor_preserve_exact_receipt_and_journal() {
    bounded(async {
        for lose_reply in [false, true] {
            let f = Fixture::with_summary_gate().await;
            let _headers = ReleaseHeaders(f.gates.clone());
            f.gates[0].release();
            let loaded = f.load(0).await.unwrap().unwrap();
            let id = loaded.conversation_id().clone();
            let live = loaded.inspect_runtime().unwrap();
            complete_history_turn(&live).await;
            let canonical = live
                .tool_runtime()
                .durable_store()
                .load_canonical()
                .unwrap();
            let weak = live.weak_inner();
            let client = loaded.client();
            let (attachment, pin) = client.attach().unwrap();
            let ParkedCompactionReply {
                reply,
                admitted,
                completed,
                release,
            } = park_transferred_reply(&client, &attachment);
            admitted.await.unwrap();
            f.gates[1].wait_entered().await;
            assert_no_manager_operation(&f, &id);
            let mut reply = Some(reply);
            if lose_reply {
                drop(reply.take());
            }
            // Disconnect while inference is physically in flight. Only an
            // explicit unload, not a lost observer or reply, owns cancellation.
            drop(attachment);
            drop(pin);
            assert!(live.has_manual_compaction());
            f.gates[1].release();
            let a = compaction_receipt(completed.await.unwrap());
            assert_eq!(a.generation, 1);
            let original = journal(&live);
            let operation = assert_one_summary_chain(&original, true, true);
            let store = live.tool_runtime().durable_store();
            let checkpoint = store.projection_head().unwrap().unwrap();
            assert_eq!(checkpoint.candidate.checkpoint_id, a.checkpoint_id);
            assert_eq!(checkpoint.activation.activation, a.activation_id);
            let generation = checkpoint.candidate.summary_generation.as_ref().unwrap();
            let settled_call = original
                .iter()
                .find(|event| matches!(event.event, RuntimeEvent::AuxiliaryCallSettled { .. }))
                .unwrap();
            assert_eq!(&generation.conversation_id, &id);
            assert_eq!(
                Some(&generation.call_id),
                settled_call.event.auxiliary_call_id()
            );
            assert_eq!(generation.through_journal, settled_call.sequence);
            let native_receipt = store
                .projection_receipt(&operation, &checkpoint.fingerprint)
                .unwrap()
                .unwrap();
            assert!(
                !native_receipt.newly_committed,
                "reading a receipt is not a second commit"
            );
            assert_eq!(&native_receipt.completed, original.last().unwrap());
            assert_eq!(
                live.tool_runtime()
                    .durable_store()
                    .load_canonical()
                    .unwrap(),
                canonical
            );
            drop(store);
            drop(live);

            // The old RPC still cannot deliver, yet the real allocation must
            // retire and a new writer must recover exactly the original facts.
            f.manager.unload(&id).await.unwrap();
            assert!(
                weak.upgrade().is_none(),
                "the parked result retains no old writer"
            );
            let successor = f.load(0).await.unwrap().unwrap();
            assert_ne!(loaded.incarnation_id(), successor.incarnation_id());
            assert!(matches!(
                client.validate(),
                Err(RuntimeManagerError::StaleIncarnation)
            ));
            let current = successor.inspect_runtime().unwrap();
            assert_eq!(journal(&current), original);
            assert_eq!(
                current
                    .tool_runtime()
                    .durable_store()
                    .load_canonical()
                    .unwrap(),
                canonical
            );
            let recovered = current
                .tool_runtime()
                .durable_store()
                .projection_receipt(&operation, &checkpoint.fingerprint)
                .unwrap()
                .unwrap();
            assert_eq!(recovered, native_receipt);
            assert_compaction_snapshot(&successor.client(), Some(&a), 1);
            assert_eq!(f.provider.request_bodies().len(), 2);

            // B is a real new compaction after cold load, not a synthetic
            // projection edit. Releasing A must still return A, never latest B.
            complete_history_turn(&current).await;
            let next_client = successor.client();
            let (next_attachment, _next_pin) = next_client.attach().unwrap();
            let authority = next_attachment.operation_authority().unwrap();
            let b = compaction_receipt(
                next_client
                    .start_compaction_operation(move |_| async move {
                        authority.admit_compaction_transfer(None)
                    })
                    .unwrap()
                    .await
                    .unwrap(),
            );
            assert_eq!(b.generation, 2);
            assert_ne!(b.checkpoint_id, a.checkpoint_id);
            assert_ne!(b.activation_id, a.activation_id);
            assert_compaction_snapshot(&next_client, Some(&b), 2);
            let after_b = journal(&current);
            assert_eq!(&after_b[..original.len()], original.as_slice());
            assert_one_summary_chain(&after_b[original.len()..], true, true);
            release.send(()).unwrap();
            if let Some(reply) = reply {
                assert_eq!(compaction_receipt(reply.await.unwrap()), a);
            }
            f.manager.unload(&id).await.unwrap();
            assert_eq!(
                journal(&current),
                after_b,
                "delivery/drain append no replay facts"
            );
            // A second cold load keeps B's identity. The client statistic
            // counts its one seeded completion, not the durable generation.
            let reloaded = f.load(0).await.unwrap().unwrap();
            assert_compaction_snapshot(&reloaded.client(), Some(&b), 1);
            assert_eq!(journal(&reloaded.inspect_runtime().unwrap()), after_b);
            assert_eq!(
                f.provider.request_bodies().len(),
                4,
                "two primary turns and two Summary Requests"
            );
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_summary_reply_does_not_block_unload_cancellation_or_replay_after_cold_load() {
    bounded(async {
        let f = Fixture::with_summary_gate().await;
        let _headers = ReleaseHeaders(f.gates.clone());
        f.gates[0].release();
        let loaded = f.load(0).await.unwrap().unwrap();
        let id = loaded.conversation_id().clone();
        let live = loaded.inspect_runtime().unwrap();
        complete_history_turn(&live).await;
        let canonical = live
            .tool_runtime()
            .durable_store()
            .load_canonical()
            .unwrap();
        let weak = live.weak_inner();
        let client = loaded.client();
        let (attachment, pin) = client.attach().unwrap();
        let ParkedCompactionReply {
            reply,
            admitted,
            completed,
            release,
        } = park_transferred_reply(&client, &attachment);
        admitted.await.unwrap();
        f.gates[1].wait_entered().await;
        assert_no_manager_operation(&f, &id);
        assert_eq!(f.provider.request_bodies().len(), 2);
        let started = journal(&live);
        assert!(
            started
                .iter()
                .any(|event| matches!(event.event, RuntimeEvent::AuxiliaryCallStarted { .. }))
        );
        assert!(
            !started
                .iter()
                .any(|event| matches!(event.event, RuntimeEvent::AuxiliaryCallSettled { .. }))
        );
        drop(reply);
        drop(attachment);
        drop(pin);
        drop(live);

        // Do not release the provider. Native unload must cancel the actual
        // in-flight Summary and join its required settlement without its caller.
        f.manager.unload(&id).await.unwrap();
        assert!(completed.await.unwrap().is_err());
        assert!(weak.upgrade().is_none());
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        let successor = f.load(0).await.unwrap().unwrap();
        assert_ne!(loaded.incarnation_id(), successor.incarnation_id());
        let current = successor.inspect_runtime().unwrap();
        let settled = journal(&current);
        assert_eq!(&settled[..started.len()], started.as_slice());
        assert_one_summary_chain(&settled, false, false);
        assert!(
            current
                .tool_runtime()
                .durable_store()
                .projection_head()
                .unwrap()
                .is_none()
        );
        assert_eq!(
            current
                .tool_runtime()
                .durable_store()
                .load_canonical()
                .unwrap(),
            canonical
        );
        assert_compaction_snapshot(&successor.client(), None, 0);
        release.send(()).unwrap();
        // Draining the reactivated owner proves there is no hidden resumed Call.
        f.manager.unload(&id).await.unwrap();
        assert_eq!(journal(&current), settled);
        assert_eq!(
            f.provider.request_bodies().len(),
            2,
            "cold recovery must not retry Summary"
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parked_completed_compaction_reply_does_not_retain_the_deleted_allocation() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_summary_gate().await;
        let _headers = ReleaseHeaders(f.gates.clone());
        f.gates[0].release();
        let loaded = f.load(0).await.unwrap().unwrap();
        let live = loaded.inspect_runtime().unwrap();
        complete_history_turn(&live).await;
        let weak = live.weak_inner();
        let client = loaded.client();
        let (attachment, pin) = client.attach().unwrap();
        let authority = attachment.operation_authority().unwrap();
        let (completed, release) = authority.hold_compaction_response_for_test();
        let reply = client
            .start_compaction_operation(move |_| async move {
                authority.admit_compaction_transfer(None)
            })
            .unwrap();
        f.gates[1].wait_entered().await;
        f.gates[1].release();
        let original = completed.await.unwrap();
        drop(attachment);
        drop(pin);
        drop(client);
        drop(live);
        drop(loaded);
        let SessionDeleteResult::Preview { preview } =
            f.manager.sessions.delete_preview(&f.sessions[0].id).await
        else {
            panic!("deletion preview");
        };
        let deleted = f
            .manager
            .delete_session(&f.sessions[0].id, &preview.target_revision)
            .await
            .unwrap();
        assert!(
            matches!(deleted, SessionDeleteResult::Deleted { .. }),
            "completed parked reply must not retain allocation: {deleted:?}"
        );
        assert!(
            weak.upgrade().is_none(),
            "reply owns only the native receipt, not the retired Runtime"
        );
        release.send(()).unwrap();
        let receipt = compaction_receipt(reply.await.unwrap());
        assert_eq!(receipt.generation, original.generation);
        assert_eq!(receipt.checkpoint_id, original.checkpoint_id);
        assert_eq!(receipt.activation_id, original.activation_id);
        assert_eq!(receipt.tokens_before, original.tokens_before);
        assert_eq!(
            receipt.estimated_tokens_after,
            original.estimated_tokens_after
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_retirement_releases_store_before_result_and_deletion() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        let f = Fixture::with_summary_gate().await;
        let _headers = ReleaseHeaders(f.gates.clone());
        f.gates[0].release();
        let loaded = f.load(0).await.unwrap().unwrap();
        let live = loaded.inspect_runtime().unwrap();
        complete_history_turn(&live).await;
        let weak = live.weak_inner();
        let compaction = tokio::spawn({
            let live = live.clone();
            async move { live.compact_context(None).await }
        });
        f.gates[1].wait_entered().await;
        let (reaped, release) = live.hold_auxiliary_retirement_for_test();
        f.gates[1].release();
        reaped.await.unwrap();
        let receipt = compaction.await.unwrap().unwrap();
        assert_eq!(
            receipt.generation, 1,
            "business compaction completes while its reaper is still parked"
        );
        drop(live);
        drop(loaded);
        let SessionDeleteResult::Preview { preview } =
            f.manager.sessions.delete_preview(&f.sessions[0].id).await
        else {
            panic!("deletion preview");
        };
        let deleted = f
            .manager
            .delete_session(&f.sessions[0].id, &preview.target_revision)
            .await
            .unwrap();
        assert!(
            matches!(deleted, SessionDeleteResult::Deleted { .. }),
            "completed retirement must release Store before notification: {deleted:?}"
        );
        assert!(weak.upgrade().is_none());
        release.send(()).unwrap();
        f.close().await;
    })
    .await;
}
