//! Native load single-flight and composition: shared and isolated flights,
//! overlapping conversations, cold reloads without replay and composition
//! failures that never touch another runtime.
#![allow(clippy::too_many_lines)]
use super::*;
use crate::durable::CanonicalStore;
use crate::durable::JournalStore;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_id_single_flight_and_cancelled_claimant() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        let a = f.load(0);
        probe.before_compose.entered().await;
        let b = f.load(0);
        probe.joined(2).await;
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(!a.is_finished() && !b.is_finished());
        a.abort();
        assert!(a.await.unwrap_err().is_cancelled());
        let c = f.load(0);
        probe.joined(3).await;
        probe.before_compose.release();
        let b = b.await.unwrap().unwrap();
        let c = c.await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&b, &c));
        assert!(f.manager.is_current(&id, b.incarnation_id()));
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_flight_is_shared_retryable_and_isolated_from_running_b() {
    bounded(async {
        let f = Fixture::new().await;
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        std::fs::write(f.workspaces[0].join("rustx.toml"), "invalid = [").unwrap();
        let a = f.load(0);
        probe.before_compose.entered().await;
        let second = f.load(0);
        probe.joined(2).await;
        probe.before_compose.release();
        let error = a.await.unwrap().unwrap_err();
        assert_eq!(error, second.await.unwrap().unwrap_err());
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 1);
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let settled = live_b.settlement_signal().notified();
        f.gates[1].release();
        settled.await;
        std::fs::remove_file(f.workspaces[0].join("rustx.toml")).unwrap();
        f.load(0).await.unwrap().unwrap();
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn different_conversations_overlap_provider_and_keep_durable_state_isolated() {
    bounded(async {
        let f = Fixture::new().await;
        let a = f.load(0).await.unwrap().unwrap();
        let live_a = a.inspect_runtime().unwrap();
        let (attachment, _) = a
            .resident
            .upgrade()
            .unwrap()
            .composition
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .host()
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        live_a.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        // The actual admitted attachment disconnects during the provider turn.
        drop(attachment);
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        assert_eq!(f.manager.diagnostics().active_roots, 2);
        assert_eq!(f.manager.diagnostics().loaded, 2);
        assert_eq!(f.provider.request_bodies().len(), 2);
        assert_eq!(
            f.manager.residency(a.conversation_id()),
            ResidencyState::Loaded
        );
        assert_eq!(
            f.manager.residency(b.conversation_id()),
            ResidencyState::Loaded
        );
        assert_ne!(a.workspace_identity(), b.workspace_identity());
        assert!(!Arc::ptr_eq(
            &live_a.runtime_resources(),
            &live_b.runtime_resources()
        ));
        for (runtime, marker) in [(&live_a, "A"), (&live_b, "B")] {
            assert!(
                runtime
                    .tool_runtime()
                    .environment()
                    .child_environment(runtime.tool_runtime().workspace().root())
                    .contains(&("RESIDENCY_SESSION".to_owned(), marker.to_owned()))
            );
            assert!(
                runtime
                    .tool_runtime()
                    .durable_store()
                    .read_events(None, 256)
                    .unwrap()
                    .events
                    .iter()
                    .all(|event| &event.conversation_id == runtime.conversation_id())
            );
        }
        assert_eq!(live_a.tool_runtime().workspace().root(), f.workspaces[0]);
        assert_eq!(live_b.tool_runtime().workspace().root(), f.workspaces[1]);
        let b_done = live_b.settlement_signal().notified();
        f.gates[1].release();
        b_done.await;
        assert!(
            !events(&live_a)
                .iter()
                .any(|e| matches!(e, RuntimeEvent::AttemptCompleted { .. }))
        );
        assert!(
            live_a.has_current_attempt(),
            "A still owns its blocked provider turn after B completes"
        );
        assert!(
            events(&live_b)
                .iter()
                .any(|e| matches!(e, RuntimeEvent::AttemptCompleted { .. }))
        );
        let a_done = live_a.settlement_signal().notified();
        f.gates[0].release();
        a_done.await;
        assert_eq!(
            events(&live_a)
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::AttemptCompleted { .. }))
                .count(),
            1
        );
        let history_a = live_a.historical_canonical_history().unwrap();
        let history_b = live_b.historical_canonical_history().unwrap();
        assert!(format!("{history_a:?}").contains("request-A"));
        assert!(!format!("{history_a:?}").contains("request-B"));
        assert!(format!("{history_b:?}").contains("request-B"));
        assert!(!format!("{history_b:?}").contains("request-A"));
        let (_reattached, _) = a
            .resident
            .upgrade()
            .unwrap()
            .composition
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .host()
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        assert_ne!(live_a.conversation_id(), live_b.conversation_id());
        assert_ne!(
            live_a
                .tool_runtime()
                .durable_store()
                .read_events(None, 256)
                .unwrap()
                .events,
            live_b
                .tool_runtime()
                .durable_store()
                .read_events(None, 256)
                .unwrap()
                .events
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_reload_recovers_accepted_history_without_another_provider_request_or_result() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let a = f.load(0).await.unwrap().unwrap();
        let live = a.inspect_runtime().unwrap();
        live.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        let done = live.settlement_signal().notified();
        f.gates[0].release();
        done.await;
        let history = live.historical_canonical_history().unwrap();
        let journal = events(&live);
        f.manager.unload(a.conversation_id()).await.unwrap();
        let resumed = f.load(0).await.unwrap().unwrap();
        let current = resumed.inspect_runtime().unwrap();
        assert_ne!(a.incarnation_id(), resumed.incarnation_id());
        assert_eq!(current.historical_canonical_history().unwrap(), history);
        assert_eq!(events(&current), journal);
        assert_eq!(f.provider.request_bodies().len(), 1);
        // Drain is the liveness proof that activation had no hidden accepted
        // work to execute; no elapsed-time or empty poll establishes this.
        f.manager.unload(resumed.conversation_id()).await.unwrap();
        assert_eq!(f.provider.request_bodies().len(), 1);
        assert_eq!(
            events(&current)
                .iter()
                .filter(|e| matches!(e, RuntimeEvent::AttemptCompleted { .. }))
                .count(),
            1
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicked_composition_task_cleans_flight_and_warm_load_never_reresolves() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        probe.before_compose.arm();
        probe.panic_once.store(true, Ordering::SeqCst);
        let first = f.load(0);
        probe.before_compose.entered().await;
        let second = f.load(0);
        probe.joined(2).await;
        probe.before_compose.release();
        let failure = first.await.unwrap().unwrap_err();
        assert_eq!(failure, second.await.unwrap().unwrap_err());
        assert_eq!(f.manager.residency(&id), ResidencyState::Unloaded);
        let loaded = f.load(0).await.unwrap().unwrap();
        std::fs::write(f.workspaces[0].join("rustx.toml"), "invalid = [").unwrap();
        // The durable allocation authority refuses a second process owner;
        // cloned manager handles share this owner's live registry.
        assert!(
            SessionRuntimeManager::new(
                f.manager.sessions.clone(),
                f.manager.configuration.clone(),
                f.manager.credentials.clone(),
                LocalRuntimeDependencies::default(),
                f.manager.policy(),
            )
            .is_err()
        );
        let other = f.manager.clone();
        let warm = other.load(&f.sessions[0].id, None).await.unwrap();
        assert!(Arc::ptr_eq(&loaded, &warm));
        assert_eq!(probe.compositions.load(Ordering::SeqCst), 2);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_composition_failure_does_not_touch_running_b() {
    bounded(async {
        let f = Fixture::new().await;
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        let access = f
            .manager
            .sessions
            .acquire_session(&f.sessions[0].id, None)
            .await
            .unwrap();
        // Real recovery/composition failure, beyond successful configuration.
        std::fs::write(&access.database_path, b"not a SQLite database").unwrap();
        assert!(f.load(0).await.unwrap().is_err());
        assert_eq!(
            f.manager.residency(&access.node.conversation_id),
            ResidencyState::Unloaded
        );
        assert!(
            f.manager
                .is_current(b.conversation_id(), b.incarnation_id())
        );
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        f.manager.unload(b.conversation_id()).await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_load_preserves_unknown_external_tool_outcome_without_replay() {
    bounded(async {
        use crate::durable::{SqliteConversationStore};
        use crate::events::types::{RuntimeEventEnvelope, EVENT_SCHEMA_VERSION};
        use crate::runtime::identity::{AttemptId, EventId, MessageId, ToolCallId, ToolId, TurnId};
        use crate::message::types::{MessageBlock, AssistantMessageBlock, AssistantContentBlock};
        use crate::tools::types::{ToolCall, ToolExecutionStatus};
        let f = Fixture::with_session_count(None, 1).await;
        let access = f.manager.sessions.acquire_session(&f.sessions[0].id, None).await.unwrap();
        let id = access.node.conversation_id.clone();
        let attempt = AttemptId::for_conversation(&id, 0);
        let envelope = |key: &str, event| RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
 schema_version: EVENT_SCHEMA_VERSION, event_id: EventId::new(key), sequence: 0,
            conversation_id: id.clone(), attempt_id: Some(attempt.clone()), turn_id: Some(TurnId::new("0")),
            timestamp: chrono::DateTime::from_timestamp(1, 0).unwrap(), event,
        };
        {
            // Exact committed crash prefix through the same SQLite authority
            // used by existing durable recovery contracts; no manager recovery.
            let store = SqliteConversationStore::open(id.clone(), &access.database_path).unwrap().with_lifecycle(access.allocation.clone());
            store.append_event(envelope("attempt", RuntimeEvent::AttemptStarted { attempt_id: attempt.clone() })).unwrap();
            let assistant = MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("assistant"), content: vec![AssistantContentBlock::ToolCall(ToolCall {
                    id: ToolCallId::new("call"), tool_id: ToolId::new("tool-write"), name: "write".into(),
                    arguments: serde_json::json!({"path":"must-not-exist", "content":"never replay"}),
                })],
            });
            store.append_canonical_with_event(&assistant, envelope("assistant", RuntimeEvent::AssistantMessageCommitted { message_id: MessageId::new("assistant") })).unwrap();
            store.append_event(envelope("tool", RuntimeEvent::ToolExecutionStarted { tool_call_id: ToolCallId::new("call"), tool_id: ToolId::new("tool-write") })).unwrap();
        }
        drop(access);
        let loaded = f.load(0).await.unwrap().unwrap(); let live = loaded.inspect_runtime().unwrap();
        let history = live.historical_canonical_history().unwrap();
        assert_eq!(history.iter().filter(|m| matches!(m, MessageBlock::Tool(tool) if matches!(tool.result.status, ToolExecutionStatus::OutcomeUnknown { .. }))).count(), 1);
        f.manager.unload(&id).await.unwrap();
        let reload = f.load(0).await.unwrap().unwrap(); let current = reload.inspect_runtime().unwrap();
        f.manager.unload(&id).await.unwrap();
        assert_eq!(current.historical_canonical_history().unwrap(), history);
        assert_eq!(events(&current).iter().filter(|e| matches!(e, RuntimeEvent::ToolExecutionStarted { .. })).count(), 1);
        assert!(f.provider.request_bodies().is_empty());
        assert!(!f.workspaces[0].join("must-not-exist").exists());
        f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_activation_never_holds_the_global_registry_lock() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        let f = Fixture::new().await;
        let id = f.id(0).await;
        let probe = f.manager.probe(&id);
        let gate = Arc::new(Gate::default());
        let release = gate.arm_scoped();
        *probe.activation.lock().unwrap() = Some(gate.clone());
        let a = f.load(0);
        gate_entered(&gate).await;
        assert_eq!(f.manager.residency(&id), ResidencyState::Loading);
        let another_a = f.load(0);
        probe.joined(2).await;
        assert!(!another_a.is_finished());
        let b = f.load(1).await.unwrap().unwrap();
        let live_b = b.inspect_runtime().unwrap();
        live_b.submit_inbound(input("request-B")).unwrap();
        f.gates[1].wait_entered().await;
        let done = live_b.settlement_signal().notified();
        f.gates[1].release();
        done.await;
        f.manager.unload(b.conversation_id()).await.unwrap();
        assert!(!a.is_finished());
        drop(release);
        let a = a.await.unwrap().unwrap();
        let another_a = another_a.await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&a, &another_a));
        f.close().await;
    })
    .await;
}

/// Initialization cancellation and the native Inactive->Running transition
/// choose one winner before durable recovery can admit any provider request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_drain_and_native_activation_have_both_initialization_winners() {
    bounded(async {
        use crate::runtime::conversation_runtime::Gate;
        // Before native activation, after native activation before admission,
        // and after the recovered request actually starts.
        for cut in [0_u8, 1, 2] {
            let f = Fixture::with_session_count(None, 1).await;
            let session = &f.sessions[0];
            {
                let access = f
                    .manager
                    .sessions
                    .acquire_session(&session.id, None)
                    .await
                    .unwrap();
                let store = crate::durable::SqliteConversationStore::open(
                    session.active_conversation_id.clone(),
                    &access.database_path,
                )
                .unwrap();
                crate::durable::inbox::InboxStore::accept_inbound(
                    &store,
                    crate::durable::InboundDraft {
                        message_id: None,
                        source: crate::message::types::UserSource::Human,
                        kind: crate::message::types::InboundKind::Message,
                        content: input("request-B: accepted work awaiting native recovery"),
                        timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                        correlation: None,
                    },
                )
                .unwrap();
            }
            let probe = f.manager.probe(&session.active_conversation_id);
            let activation = Arc::new(Gate::default());
            let release = activation.arm_scoped();
            if cut == 1 {
                *probe.initialization_admission.lock().unwrap() = Some(activation.clone());
            } else {
                *probe.activation.lock().unwrap() = Some(activation.clone());
            }
            let loading = f.load(0);
            gate_entered(&activation).await;
            assert!(f.provider.request_bodies().is_empty());
            if cut < 2 {
                let native = {
                    let registry = f.manager.registry.0.lock().unwrap();
                    let Entry::Loading(flight) = &registry.entries[&session.active_conversation_id]
                    else {
                        panic!("activation or its first admission is still held");
                    };
                    flight.initialization.prepared_runtime().unwrap()
                };
                assert_eq!(native.is_activated(), cut == 1);
                // The host returns only after generic native retirement owns
                // the lifecycle, including the post-CAS recovery handoff.
                f.host.begin_drain();
                assert!(!native.title_lifecycle().is_running());
                initialization_cancelled(&f, &session.active_conversation_id).await;
                let host = f.host.clone();
                let draining = tokio::spawn(async move { host.drain().await });
                drop(release);
                assert!(matches!(
                    loading.await.unwrap(),
                    Err(RuntimeManagerError::LoadingCancelled)
                ));
                assert!(draining.await.unwrap().is_empty());
                assert!(f.provider.request_bodies().is_empty());
            } else {
                drop(release);
                loading.await.unwrap().unwrap();
                f.gates[1].wait_entered().await;
                assert_eq!(f.provider.request_bodies().len(), 1);
                f.host.begin_drain();
                let host = f.host.clone();
                let draining = tokio::spawn(async move { host.drain().await });
                f.gates[1].release();
                assert!(draining.await.unwrap().is_empty());
                assert_eq!(f.provider.request_bodies().len(), 1);
            }
            assert!(f.manager.is_empty());
            f.close().await;
        }
    })
    .await;
}
