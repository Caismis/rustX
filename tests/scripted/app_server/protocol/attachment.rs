//! Attachment routes: reservation, capacity, detach, close and stale-route
//! fencing, plus subscription registration across attach receipts.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_during_unloading_needs_no_operation_lease() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let probe = f.manager.probe(&old.conversation_id);
        // The attachment helper's independent checkpoint already used this gate.
        // Rearm it: a wrongly admitted detach would now park instead of passing.
        probe.before_operation.arm();
        probe.before_shutdown.arm();
        let unload = super::super::unload_task(&f, old.conversation_id.clone());
        probe.before_shutdown.entered().await;
        assert_eq!(
            f.manager.residency(&old.conversation_id),
            super::super::ResidencyState::Unloading
        );
        assert!(matches!(
            call(&connection, 1, Method::SessionDetach { target: old }).await,
            MethodResult::Detached {}
        ));
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(
            !*probe.before_operation.entered.borrow(),
            "detach admits no operation lease"
        );
        probe.before_operation.release();
        probe.before_shutdown.release();
        unload.await.unwrap().unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_detach_cannot_remove_replacement_route() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        call(
            &connection,
            1,
            Method::SessionDetach {
                target: old.clone(),
            },
        )
        .await;
        let new = attach(&connection, &f, 0).await;
        assert_ne!(old.attachment_id, new.attachment_id);
        assert_eq!(old.runtime_incarnation, new.runtime_incarnation);
        assert_eq!(
            rejected(&connection, Method::SessionDetach { target: old }).await,
            ErrorData::StaleAttachment
        );
        assert_eq!(connection.attachment_counts(), (1, 0));
        call(
            &connection,
            2,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: new,
            },
        )
        .await;
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_async_operation_drains_before_delete_releases_resources() {
    bounded(async {
        use crate::local_runtime::session::deletion::SessionDeleteResult;
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let identity = f.manager.load(&target.session_id, None).await.unwrap();
        let weak_runtime = identity.inspect_runtime().unwrap().weak_inner();
        let weak_host = {
            let resident = identity.resident.upgrade().unwrap();
            let composition = resident.composition.lock().unwrap();
            composition.as_ref().unwrap().host().weak_inner()
        };
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let worker = connection.clone();
        let operation_target = target.clone();
        let operation = tokio::spawn(async move {
            let response = worker
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(200),
                    call: Method::ConfigurationGet {
                        target: operation_target,
                    },
                })
                .await;
            let wire = serde_json::to_value(&response).unwrap();
            assert!(
                jsonschema::validator_for(&crate::app_server::schema::protocol_schema())
                    .unwrap()
                    .is_valid(&wire)
            );
            assert!(
                serde_json::to_vec(&wire).unwrap().len()
                    <= crate::app_server::transport::MAX_MESSAGE_BYTES
            );
            let Response::Success(receipt) = response else {
                panic!("admitted original bounded read: {response:?}");
            };
            assert_eq!(receipt.id, RequestId::Integer(200));
            receipt.result
        });
        probe.before_operation.entered().await;
        let worker = connection.clone();
        let MethodResult::Deletion {
            result: crate::app_server::session_deletion::SessionDeletionResult::Preview { preview },
        } = call(
            &connection,
            199,
            Method::SessionDeletePreview {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("preview")
        };
        let delete_target = target.clone();
        let delete = tokio::spawn(async move {
            call(
                &worker,
                201,
                Method::SessionDelete {
                    session_id: delete_target.session_id,
                    expected_target_revision: preview.target_revision,
                },
            )
            .await
        });
        probe
            .draining_operations
            .subscribe()
            .wait_for(|draining| *draining)
            .await
            .unwrap();
        assert_eq!(
            f.manager.residency(&target.conversation_id),
            super::super::ResidencyState::Unloading
        );
        assert!(
            !delete.is_finished(),
            "unload is waiting on the admitted operation, not the client"
        );
        assert!(matches!(
            call(
                &connection,
                202,
                Method::SessionRecoverDeletion {
                    session_id: target.session_id.clone()
                }
            )
            .await,
            MethodResult::Deletion {
                result: crate::app_server::session_deletion::SessionDeletionResult::Preview { .. }
            }
        ));
        assert!(
            f.manager
                .registry
                .0
                .lock()
                .unwrap()
                .retiring_sessions
                .contains(&target.session_id)
        );
        assert!(weak_runtime.upgrade().is_some());
        assert_eq!(
            rejected(
                &connection,
                Method::TurnStart {
                    target: target.clone(),
                    content: (input("must not run"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) =>
                                crate::app_server::protocol::UserInputBlock::Text(text),
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect()
                }
            )
            .await,
            ErrorData::StaleRuntime
        );
        probe.before_operation.release();
        let MethodResult::EffectiveConfiguration { projection } = operation.await.unwrap() else {
            panic!("the admitted original configuration read succeeds during retirement");
        };
        assert!(!projection.revision.is_empty());
        assert!(
            projection.inline.is_some() || !projection.fields.is_empty(),
            "the bounded receipt retains original selected-field evidence"
        );
        assert!(matches!(
            delete.await.unwrap(),
            MethodResult::Deletion { .. }
        ));
        assert!(weak_host.upgrade().is_none());
        assert!(weak_runtime.upgrade().is_none());
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            matches!(
                f.manager.sessions.delete_preview(&target.session_id).await,
                SessionDeleteResult::NotFound { .. }
            ),
            "deletion committed after all admitted operations drained"
        );
        f.close().await;
    })
    .await;
}

async fn cold_sessions(
    f: &Fixture,
    count: usize,
) -> Vec<crate::local_runtime::session::SessionSnapshot> {
    let mut sessions = Vec::new();
    for _ in 0..count {
        sessions.push(
            f.manager
                .sessions
                .create_session(
                    crate::local_runtime::session::SessionPersistentState::from_input(
                        &crate::local_runtime::configuration::SessionConfigInput::new(
                            f.workspaces[0].clone(),
                        ),
                    ),
                )
                .await
                .unwrap()
                .session,
        );
    }
    sessions
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachment_capacity_rejection_never_composes_and_detach_reclaims_without_notifications() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new_with_attachment_limit_for_test(f.host.clone(), 2);
        initialize(&connection).await;
        let sessions = cold_sessions(&f, 3).await;
        assert_eq!(connection.attachment_counts(), (0, 0));
        let mut targets = Vec::new();
        for session in &sessions[..2] {
            targets.push(attach_session(&connection, session).await);
        }
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        assert_eq!(
            rejected(
                &connection,
                Method::SessionAttach {
                    session_id: sessions[2].id.clone(),
                    node_id: None
                }
            )
            .await,
            ErrorData::AttachmentCapacity
        );
        assert_eq!(
            f.manager
                .probe(&sessions[2].active_conversation_id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            f.manager.residency(&sessions[2].active_conversation_id),
            super::super::ResidencyState::Unloaded
        );
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        for target in targets.drain(..1) {
            call(&connection, 301, Method::SessionDetach { target }).await;
        }
        assert_eq!(connection.attachment_counts(), (1, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 1);
        // No next_notification call: unload's response is the terminal acknowledgement.
        for session in &sessions[2..] {
            targets.push(attach_session(&connection, session).await);
        }
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        assert_eq!(
            f.manager
                .probe(&sessions[2].active_conversation_id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(f.provider.request_bodies().is_empty());
        for target in targets {
            call(&connection, 302, Method::SessionDetach { target }).await;
        }
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        for session in &sessions {
            assert_eq!(
                f.manager.residency(&session.active_conversation_id),
                super::super::ResidencyState::Loaded
            );
        }
        connection.close();
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_final_slot_is_reserved_before_composition() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new_with_attachment_limit_for_test(
            f.host.clone(),
            2,
        ));
        initialize(&connection).await;
        let sessions = cold_sessions(&f, 3).await;
        for session in &sessions[..1] {
            attach_session(&connection, session).await;
        }
        assert_eq!(connection.attachment_counts(), (1, 0));
        let probes = [
            f.manager.probe(&sessions[1].active_conversation_id),
            f.manager.probe(&sessions[2].active_conversation_id),
        ];
        for probe in &probes {
            probe.before_compose.arm();
        }
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for (index, session) in sessions[1..].iter().enumerate() {
            let worker = connection.clone();
            let barrier = barrier.clone();
            let session_id = session.id.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                worker
                    .handle_request(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(400 + i64::try_from(index).unwrap()),
                        call: Method::SessionAttach {
                            session_id,
                            node_id: None,
                        },
                    })
                    .await
            }));
        }
        barrier.wait().await;
        let winner = tokio::select! {
            () = probes[0].before_compose.entered() => 0,
            () = probes[1].before_compose.entered() => 1,
        };
        assert_eq!(connection.attachment_counts(), (1, 1));
        let loser = tasks.remove(1 - winner).await.unwrap();
        assert!(matches!(
            loser,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::AttachmentCapacity),
                    ..
                },
                ..
            })
        ));
        assert_eq!(
            probes[1 - winner]
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            f.manager
                .residency(&sessions[2 - winner].active_conversation_id),
            super::super::ResidencyState::Unloaded
        );
        assert_eq!(connection.attachment_counts(), (1, 1));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        probes[winner].before_compose.release();
        assert!(matches!(
            tasks.remove(0).await.unwrap(),
            Response::Success(_)
        ));
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert!(f.provider.request_bodies().is_empty());
        for session in &sessions {
            f.manager
                .unload(&session.active_conversation_id)
                .await
                .unwrap();
        }
        connection.close();
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        connection.close();
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_attach_releases_reservation_but_not_manager_owned_load() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.before_compose.arm();
        let worker = connection.clone();
        let session = f.sessions[0].clone();
        let attaching = tokio::spawn(async move { attach_session(&worker, &session).await });
        probe.before_compose.entered().await;
        assert_eq!(connection.attachment_counts(), (0, 1));
        attaching.abort();
        assert!(attaching.await.unwrap_err().is_cancelled());
        assert_eq!(connection.attachment_counts(), (0, 0));
        // Observe the already-claimed flight without initiating another load.
        let id = f.id(0).await;
        let flight = {
            let registry = f.manager.registry.0.lock().unwrap();
            let super::super::Entry::Loading(flight) = registry.entries.get(&id).unwrap() else {
                panic!("manager-owned Loading flight")
            };
            flight.clone()
        };
        probe.before_compose.release();
        let resident = flight.wait().await.operation_result().unwrap().unwrap();
        assert_eq!(
            f.manager.residency(resident.conversation_id()),
            super::super::ResidencyState::Loaded
        );
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let target = attach(&connection, &f, 0).await;
        assert_eq!(target.runtime_incarnation, resident.incarnation_id());
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        call(
            &connection,
            500,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        let fresh = attach(&connection, &f, 0).await;
        assert_ne!(fresh.attachment_id, target.attachment_id);
        assert_eq!(fresh.runtime_incarnation, target.runtime_incarnation);
        assert_eq!(
            probe.compositions.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        let target = fresh;
        probe.before_operation.arm();
        let worker = connection.clone();
        let operation_target = target.clone();
        let operation = tokio::spawn(async move {
            call(
                &worker,
                501,
                Method::ConfigurationGet {
                    target: operation_target,
                },
            )
            .await
        });
        probe.before_operation.entered().await;
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        let unload = super::super::unload_task(&f, target.conversation_id.clone());
        probe
            .draining_operations
            .subscribe()
            .wait_for(|v| *v)
            .await
            .unwrap();
        assert!(!unload.is_finished());
        probe.before_operation.release();
        unload.await.unwrap().unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_connection_pipelines_sessions_without_cross_routing_and_detach_keeps_execution() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let (a, b) = tokio::join!(attach(&connection, &f, 0), attach(&connection, &f, 1));
        // Both root runtimes were composed with no ordinary user boundary,
        // so each armed the one-shot display-projection publisher (Issue
        // #386). Awaiting its probe is the deterministic replacement for
        // observing the derived preview at list time.
        let mut projection_probe =
            crate::local_runtime::session_display_projection::display_projection_probe(
                &a.session_id,
            )
            .expect("root composition armed the display-projection publisher");
        let (reply_a, reply_b) = tokio::join!(
            call(
                &connection,
                20,
                Method::TurnStart {
                    target: a.clone(),
                    content: (input("request-A"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) =>
                                crate::app_server::protocol::UserInputBlock::Text(text),
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect()
                }
            ),
            call(
                &connection,
                21,
                Method::TurnStart {
                    target: b.clone(),
                    content: (input("request-B"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) =>
                                crate::app_server::protocol::UserInputBlock::Text(text),
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect()
                }
            ),
        );
        assert!(matches!(reply_a, MethodResult::InboundAccepted { .. }));
        assert!(matches!(reply_b, MethodResult::InboundAccepted { .. }));
        tokio::join!(f.gates[0].wait_entered(), f.gates[1].wait_entered());
        f.manager
            .configuration_host(&a.session_id)
            .unwrap()
            .projection_settled()
            .await;
        let frozen = f.manager.configuration_host(&a.session_id).unwrap();
        frozen.inner.park_projection_worker();
        let before = call(
            &connection,
            22,
            Method::SessionCheckpoint {
                target: a.clone(),
                trace_records: Vec::new(),
            },
        )
        .await;
        let residency_before = f.manager.diagnostics();
        // The preview is published by the armed one-shot publisher after the
        // canonical root-lineage commit, which provably precedes the parked
        // provider request the gates observed above. Await the publisher
        // deterministically — no sleeps.
        projection_probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(
            projection_probe.borrow().published,
            "the first canonical ordinary user commit published the projection"
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            23,
            Method::SessionSummary {
                session_id: a.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary during execution")
        };
        assert_eq!(summary.id, a.session_id);
        assert_eq!(summary.preview.as_deref(), Some("request-A"));
        assert_eq!(f.manager.diagnostics(), residency_before);
        let after = call(
            &connection,
            24,
            Method::SessionCheckpoint {
                target: a.clone(),
                trace_records: Vec::new(),
            },
        )
        .await;
        let (
            MethodResult::Checkpoint {
                checkpoint: checkpoint_1,
            },
            MethodResult::Checkpoint {
                checkpoint: checkpoint_2,
            },
        ) = (before, after)
        else {
            panic!("snapshots");
        };
        let before = Box::new(checkpoint_1.snapshot);
        let before_cursor = checkpoint_1.cursor;
        let after = Box::new(checkpoint_2.snapshot);
        let after_cursor = checkpoint_2.cursor;
        assert_eq!(before, after, "catalog summary changes no Session field");
        assert!(after_cursor >= before_cursor);
        frozen.inner.unpark_projection_worker();
        let mut checked_cursor = before_cursor;
        let mut seen = std::collections::BTreeSet::new();
        while seen.len() != 2 || checked_cursor < after_cursor {
            let notification = connection.next_notification().await.notification;
            // Session metadata invalidation is not a Conversation event: it
            // carries no cursor and no attachment target (Issue #386); nor is
            // a source-scoped repair level (Issue #464).
            if matches!(
                notification,
                NotificationMethod::SummaryInvalidated { .. }
                    | NotificationMethod::SourceRepairChanged { .. }
            ) {
                continue;
            }
            let NotificationMethod::Event {
                target,
                event,
                cursor,
                ..
            } = notification
            else {
                panic!("event")
            };
            assert!(target == a || target == b);
            if target == a && cursor > before_cursor && cursor <= after_cursor {
                assert_eq!(cursor.get(), checked_cursor.get() + 1);
                assert!(event.facts().iter().all(|fact| matches!(
                    fact,
                    RuntimeClientEvent::ReadDomainsUpdated { .. }
                        | RuntimeClientEvent::TraceChanged
                )));

                checked_cursor = cursor;
            }
            if event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
            {
                seen.insert(target.session_id.clone());
            }
            if let RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } = &*event
                && checkpoint.snapshot.attempt.as_ref().is_some_and(|attempt| {
                    matches!(
                        attempt.phase,
                        crate::runtime_client::RuntimeClientAttemptPhase::Admitted
                            | crate::runtime_client::RuntimeClientAttemptPhase::Running
                    )
                })
            {
                assert_eq!(checkpoint.cursor, cursor);
                assert_eq!(checkpoint.read_cut.conversation_id, target.conversation_id);
                seen.insert(target.session_id.clone());
            }
        }
        let rival = AppServerConnection::new(f.host.clone());
        initialize(&rival).await;
        let response = rival
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(30),
                call: Method::SessionAttach {
                    session_id: a.session_id.clone(),
                    node_id: None,
                },
            })
            .await;
        assert!(matches!(
            response,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::ControllerInUse),
                    ..
                },
                ..
            })
        ));
        call(&connection, 31, Method::SessionDetach { target: a.clone() }).await;
        let stale = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(310),
                call: Method::TurnCancel { target: a.clone() },
            })
            .await;
        assert!(matches!(
            stale,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::StaleAttachment),
                    ..
                },
                ..
            })
        ));
        let live_a = f.manager.load(&a.session_id, None).await.unwrap();
        assert_eq!(live_a.incarnation_id(), a.runtime_incarnation);
        assert!(live_a.inspect_runtime().unwrap().has_current_attempt());
        let new_a = attach(&rival, &f, 0).await;
        assert_ne!(new_a.attachment_id, a.attachment_id);
        let MethodResult::Checkpoint {
            checkpoint: checkpoint_3,
        } = call(
            &rival,
            32,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: new_a,
            },
        )
        .await
        else {
            panic!("snapshot")
        };
        let snapshot = Box::new(checkpoint_3.snapshot);
        assert_eq!(snapshot.conversation_id, a.conversation_id);
        f.gates[1].release();
        loop {
            if let NotificationMethod::Event { target, event, .. } =
                connection.next_notification().await.notification
                && event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            {
                assert_eq!(target, b);
                break;
            }
        }
        assert!(
            live_a.inspect_runtime().unwrap().has_current_attempt(),
            "B settled while A remained blocked"
        );
        f.gates[0].release();
        drop(connection);
        drop(rival);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_connection_cannot_retain_or_control_replacement() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let identity = f.manager.load(&old.session_id, None).await.unwrap();
        let weak = identity.inspect_runtime().unwrap().weak_inner();
        f.manager.unload(&old.conversation_id).await.unwrap();
        assert!(weak.upgrade().is_none());
        let replacement = f.manager.load(&old.session_id, None).await.unwrap();
        assert_ne!(replacement.incarnation_id(), old.runtime_incarnation);
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(50),
                call: Method::TurnStart {
                    target: old,
                    content: (input("must not execute"))
                        .into_iter()
                        .map(|block| match block {
                            crate::message::types::UserContentBlock::Text(text) => {
                                crate::app_server::protocol::UserInputBlock::Text(text)
                            }
                            _ => panic!("client fixtures must use text or issued receipts"),
                        })
                        .collect(),
                },
            })
            .await;
        assert!(matches!(
            response,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::StaleRuntime),
                    ..
                },
                ..
            })
        ));
        assert!(f.provider.request_bodies().is_empty());
        drop(connection);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn t13_attach_identity_checkpoint_and_subscribe_preserve_the_native_cut() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let identity = f.manager.load(&f.sessions[0].id, None).await.unwrap();
        let runtime = identity.inspect_runtime().unwrap();
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let Response::Success(attached) = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(80),
                call: Method::SessionAttach {
                    session_id: f.sessions[0].id.clone(),
                    node_id: None,
                },
            })
            .await
        else {
            panic!("attach")
        };
        let MethodResult::Attached { target, .. } = attached.result else {
            panic!("identity receipt")
        };
        let MethodResult::Checkpoint { checkpoint } = call(
            &connection,
            81,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint")
        };
        let cursor = checkpoint.cursor;
        assert!(checkpoint.snapshot.inbound.pending.is_empty());
        // Publication between C and registration remains in native Replay;
        // explicit subscribe(C) does not jump to latest.
        let accepted = runtime.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        call(
            &connection,
            82,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: cursor,
            },
        )
        .await;
        let mut previous = cursor.get();
        let mut occurrences = 0;
        f.gates[0].release();
        loop {
            let notification = connection.next_notification().await.notification;
            // Session metadata invalidation is not a Conversation event: it
            // carries no cursor, so it never participates in this cursor
            // monotonicity check (Issue #386); nor does a source-scoped repair
            // level (Issue #464).
            if matches!(
                notification,
                NotificationMethod::SummaryInvalidated { .. }
                    | NotificationMethod::SourceRepairChanged { .. }
            ) {
                continue;
            }
            let NotificationMethod::Event {
                target: routed,
                cursor,
                event,
                ..
            } = notification
            else {
                panic!("event")
            };
            assert_eq!(routed, target);
            assert_eq!(cursor.get(), previous + 1);
            previous = cursor.get();
            let RuntimeClientEvent::ProjectionTransition { events, metadata } = *event else {
                panic!("one bounded native transition");
            };
            assert_eq!(metadata.read_cut.revision, cursor.get());
            let mut settled = false;
            for event in events {
                if let RuntimeClientEvent::InboundEnqueued { message, .. } = &event {
                    assert_eq!(message.id, accepted.message_id);
                    occurrences += 1;
                }
                settled |= matches!(event, RuntimeClientEvent::AttemptSettled { .. });
            }
            if settled {
                break;
            }
        }
        assert_eq!(
            occurrences, 1,
            "publication is delivered once after the snapshot"
        );
        let response = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(81),
                call: Method::SessionSubscribe {
                    target: target.clone(),
                    after_cursor: crate::runtime_client::types::RuntimeClientCursor::new(u64::MAX),
                },
            })
            .await;
        assert!(matches!(
            response,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::ResyncRequired),
                    ..
                },
                ..
            })
        ));
        drop(runtime);
        call(
            &connection,
            82,
            Method::SessionDetach {
                target: target.clone(),
            },
        )
        .await;
        let replacement = f.manager.replace(&target.session_id, None).await.unwrap();
        assert!(
            f.manager
                .unload_incarnation(&target.conversation_id, target.runtime_incarnation)
                .await
                .is_err()
        );
        assert!(
            f.manager
                .is_current(replacement.conversation_id(), replacement.incarnation_id())
        );
        drop(connection);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_close_is_idempotent_and_revokes_claims_despite_retained_arc() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let retained = connection.clone();
        connection.close();
        assert_eq!(connection.attachment_counts(), (0, 0));
        let replacement = AppServerConnection::new(f.host.clone());
        initialize(&replacement).await;
        let new = attach(&replacement, &f, 0).await;
        assert_eq!(old.runtime_incarnation, new.runtime_incarnation);
        assert_ne!(old.attachment_id, new.attachment_id);
        retained.close();
        assert_eq!(
            rejected(&retained, Method::SessionDetach { target: old }).await,
            ErrorData::StaleAttachment
        );
        call(
            &replacement,
            2,
            Method::SessionCheckpoint {
                trace_records: vec![],
                target: new,
            },
        )
        .await;
        replacement.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_linearizes_before_pending_attach_commit() {
    bounded(async {
        let f = Fixture::new().await;
        let probe = f.manager.probe(&f.id(0).await);
        probe.before_compose.arm();
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let pending = connection.clone();
        let session_id = f.sessions[0].id.clone();
        let request = tokio::spawn(async move {
            rejected(
                &pending,
                Method::SessionAttach {
                    session_id,
                    node_id: None,
                },
            )
            .await
        });
        probe.before_compose.entered().await;
        assert_eq!(connection.attachment_counts(), (0, 1));
        connection.close();
        probe.before_compose.release();
        assert_eq!(request.await.unwrap(), ErrorData::StaleAttachment);
        assert_eq!(connection.attachment_counts(), (0, 0));
        let replacement = AppServerConnection::new(f.host.clone());
        initialize(&replacement).await;
        attach(&replacement, &f, 0).await;
        replacement.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_connection_deletes_an_attached_idle_session_and_closes_its_route() {
    bounded(async {
        use crate::app_server::session_deletion::SessionDeletionResult as Deleted;
        let f = Fixture::new().await;
        let viewer = AppServerConnection::new(f.host.clone());
        let deleter = AppServerConnection::new(f.host.clone());
        initialize(&viewer).await;
        initialize(&deleter).await;
        let target = attach(&viewer, &f, 0).await;
        let MethodResult::Deletion {
            result: Deleted::Preview { preview },
        } = call(
            &deleter,
            900,
            Method::SessionDeletePreview {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("attached preview")
        };
        let result = call(
            &deleter,
            901,
            Method::SessionDelete {
                session_id: target.session_id.clone(),
                expected_target_revision: preview.target_revision,
            },
        )
        .await;
        assert!(matches!(
            result,
            MethodResult::Deletion {
                result: Deleted::Deleted { .. }
            }
        ));
        assert!(
            !f.manager
                .is_current(&target.conversation_id, target.runtime_incarnation)
        );
        assert!(f.manager.load(&target.session_id, None).await.is_err());
        loop {
            let notice = viewer.next_notification().await;
            if matches!(notice.notification, NotificationMethod::Closed { .. }) {
                break;
            }
        }
        assert_eq!(viewer.attachment_counts(), (0, 0));
        assert_eq!(
            rejected(&viewer, Method::TurnCancel { target }).await,
            ErrorData::StaleAttachment
        );
        f.close().await;
    })
    .await;
}

async fn retained_huge_root(fixture: &Fixture) -> Vec<(crate::runtime::identity::MessageId, u64)> {
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::MessageId;
    let session = &fixture.sessions[0];
    let access = fixture
        .manager
        .session_controller()
        .acquire_session(&session.id, None)
        .await
        .unwrap();
    let store = SqliteConversationStore::open(
        session.active_conversation_id.clone(),
        &access.database_path,
    )
    .unwrap();
    // The model admission limit is per immutable record. Keep the retained
    // aggregate above 2 MiB while each original is a legal native input.
    let mut originals = Vec::new();
    for index in 0..4 {
        let text = "水\n".repeat(150_000);
        let readable_end = u64::try_from(text.len()).unwrap();
        let message_id = MessageId::new(format!("retained-huge-root-{index}"));
        let original = MessageBlock::User(UserMessageBlock {
            id: message_id.clone(),
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
            content: vec![UserContentBlock::Text(crate::message::TextBlock { text })],
        });
        assert!(
            serde_json::to_vec(&original).unwrap().len()
                < crate::context::resources::ResourceLimits::default().record_bytes
        );
        store.append_canonical(&original).unwrap();
        originals.push((message_id, readable_end));
    }
    assert!(originals.iter().map(|(_, bytes)| bytes).sum::<u64>() > 2 * 1_048_576);
    drop(store);
    drop(access);
    originals
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t13_lost_huge_root_attach_receipt_is_reconciled_without_second_admission() {
    bounded(async {
        use std::task::Poll;
        let fixture = Fixture::new().await;
        let originals = retained_huge_root(&fixture).await;
        let session = &fixture.sessions[0];
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let reply = connection
            .reply(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(46_710),
                call: Method::SessionAttach {
                    session_id: session.id.clone(),
                    node_id: None,
                },
            })
            .await;
        let mut lost = crate::app_server::transport::Outbound::reply(reply).unwrap();
        let mut receipt_bytes = 0;
        // Attach is committed, but the physical attempt accepts no bytes.
        assert!(matches!(
            lost.poll_hand_off::<()>(|record| {
                receipt_bytes = record.len();
                Poll::Pending
            }),
            Poll::Pending
        ));
        assert!(receipt_bytes > 0 && receipt_bytes < 4096);
        drop(lost);
        let MethodResult::AttachmentStatus {
            target: Some(target),
            ..
        } = call(
            &connection,
            46_711,
            Method::SessionAttachmentStatus {
                session_id: session.id.clone(),
            },
        )
        .await
        else {
            panic!("committed attachment remains resolvable");
        };
        assert_eq!(target.conversation_id, session.active_conversation_id);
        assert_eq!(
            rejected(
                &connection,
                Method::SessionAttach {
                    session_id: session.id.clone(),
                    node_id: None,
                }
            )
            .await,
            ErrorData::ControllerInUse
        );
        let MethodResult::Checkpoint { checkpoint } = call(
            &connection,
            46_712,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("independent bounded checkpoint");
        };
        assert!(serde_json::to_vec(&checkpoint).unwrap().len() <= 786_432);
        for (message_id, readable_end) in &originals {
            let descriptor = checkpoint
                .snapshot
                .message_descriptors
                .iter()
                .find(|descriptor| descriptor.identity.as_ref() == Some(message_id))
                .unwrap();
            assert!(descriptor.content.iter().any(
                |binding| binding.deferred && binding.content_ref.readable_end == *readable_end
            ));
        }
        assert!(
            fixture.provider.request_bodies().is_empty(),
            "reconciliation starts no model work"
        );
        call(&connection, 46_713, Method::SessionDetach { target }).await;
        assert!(matches!(
            call(
                &connection,
                46_714,
                Method::SessionAttachmentStatus {
                    session_id: session.id.clone(),
                }
            )
            .await,
            MethodResult::AttachmentStatus { target: None, .. }
        ));
        connection.close();
        fixture.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t07_t13_source_failure_fences_registration_without_replay_loss_or_cursor_consumption() {
    bounded(async {
        use futures_util::FutureExt;
        use crate::durable::ConversationStore as _;
        struct SourceFault { connection: rusqlite::Connection, original: Vec<u8> }
        impl SourceFault {
            fn restore(&self) {
                assert_eq!(self.connection.execute("UPDATE bootstrap_identity SET occupancy_observation=?1 WHERE id=1",[&self.original]).unwrap(),1);
            }
        }
        impl Drop for SourceFault { fn drop(&mut self) { self.restore(); } }
        let fixture = Fixture::new().await;
        let connection = AppServerConnection::new(fixture.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &fixture, 0).await;
        let native = fixture
            .manager
            .configuration_host(&target.session_id)
            .unwrap();
        native.projection_settled().await;
        let inner = native.weak_inner().upgrade().unwrap();
        inner.park_projection_worker();
        let MethodResult::Checkpoint { checkpoint } = call(
            &connection,
            47_800,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("native checkpoint")
        };
        let MethodResult::Subscribed {
            registration_id, ..
        } = call(
            &connection,
            47_801,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: checkpoint.cursor,
            },
        )
        .await
        else {
            panic!("accepted registration")
        };
        let slot = connection.registration_for_test(&target);
        assert_eq!(slot.accepted_cursor(), Some(checkpoint.cursor));
        // Retain a real derived Source fault while explicit checkpoint repair
        // uses the original worker. Canonical/J/operation authority is untouched.
        let access=fixture.manager.session_controller().acquire_read(&target.session_id,Some(&fixture.sessions[0].active_node)).await.unwrap();
        let source=rusqlite::Connection::open(&access.database_path).unwrap();
        let store=crate::durable::SqliteConversationStore::open_existing(target.conversation_id.clone(),&access.database_path).unwrap();
        drop(access);
        let original_cut=store.observation_read_cut().unwrap();
        let original_journal=store.read_events(None,32).unwrap().events;
        let original=source.query_row("SELECT occupancy_observation FROM bootstrap_identity WHERE id=1",[],|row|row.get::<_,Vec<u8>>(0)).unwrap();
        assert_eq!(source.execute("UPDATE bootstrap_identity SET occupancy_observation=NULL WHERE id=1",[]).unwrap(),1);
        let fault=SourceFault {connection:source,original};
        native.fail_projection_read_for_test("same-fence source failure");
        loop {
            match connection.next_notification().await.notification {
                NotificationMethod::ObservationUnavailable {
                    target: found,
                    registration_id: found_registration,
                    message,
                } => {
                    assert_eq!(found, target);
                    assert_eq!(found_registration, registration_id);
                    assert!(message.contains("same-fence source failure"));
                    break;
                }
                NotificationMethod::ResyncRequired { .. }
                | NotificationMethod::Event { .. }
                | NotificationMethod::Closed { .. } => {
                    panic!("source failure is neither replay loss nor an event/close")
                }
                _ => {}
            }
        }
        assert_eq!(slot.accepted_cursor(), Some(checkpoint.cursor));
        inner.unpark_projection_worker();
        assert!(matches!(
            rejected(
                &connection,
                Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![]
                }
            )
            .await,
            ErrorData::ObservationUnavailable {diagnostic} if diagnostic.contains("inherited occupancy reading source unavailable")
        ));
        assert!(matches!(
            rejected(
                &connection,
                Method::SessionSubscribe {
                    target: target.clone(),
                    after_cursor: checkpoint.cursor
                }
            )
            .await,
            ErrorData::ObservationUnavailable {diagnostic} if diagnostic.contains("inherited occupancy reading source unavailable")
        ));
        assert!(
            connection
                .next_notification_candidate()
                .now_or_never()
                .is_none(),
            "unavailable registration is fenced, not a notification loop"
        );
        assert_eq!(slot.accepted_cursor(), Some(checkpoint.cursor));
        fault.restore();
        let MethodResult::Checkpoint {checkpoint:repaired}=call(&connection,47_803,Method::SessionCheckpoint {target:target.clone(),trace_records:vec![]}).await else {panic!("explicit public checkpoint repairs the original Source");};
        assert_eq!(repaired.cursor.get(),checkpoint.cursor.get()+1);
        assert_eq!(repaired.read_cut.source_cut,checkpoint.read_cut.source_cut);
        assert_eq!(store.observation_read_cut().unwrap(),original_cut);
        assert_eq!(store.read_events(None,32).unwrap().events,original_journal);
        let MethodResult::Subscribed {
            registration_id: successor,
            ..
        } = call(
            &connection,
            47_802,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: checkpoint.cursor,
            },
        )
        .await
        else {
            panic!("explicit recovery")
        };
        assert_ne!(successor, registration_id);
        loop {
            match connection.next_notification().await.notification {
                NotificationMethod::Event {
                    registration_id: found,
                    cursor,
                    event,
                    ..
                } => {
                    assert_eq!(found, successor);
                    assert_eq!(cursor.get(), checkpoint.cursor.get() + 1);
                    assert!(
                        event.facts().iter().any(|fact| matches!(
                            fact,
                            RuntimeClientEvent::ReadDomainsUpdated { .. }
                        ))
                    );
                    break;
                }
                NotificationMethod::ResyncRequired { .. }
                | NotificationMethod::ObservationUnavailable { .. }
                | NotificationMethod::Closed { .. } => {
                    panic!("same-fence successful repair remains continuously replayable")
                }
                _ => {}
            }
        }
        assert_eq!(
            fixture.provider.request_bodies().len(),
            0,
            "observation recovery starts no execution"
        );
        drop(fault);
        drop(store);
        drop(inner);
        drop(native);
        fixture.close().await;
    })
    .await;
}
