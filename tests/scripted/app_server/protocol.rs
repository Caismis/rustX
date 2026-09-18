//! Direct connection contracts. Provider gates prove overlap; no socket timing.
use super::{Fixture, bounded, input};
use crate::app_server::connection::AppServerConnection;
use crate::app_server::protocol::*;
use crate::runtime_client::event::RuntimeClientEvent;

use super::app_server_conformance as conformance;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_connection_runs_shared_transport_neutral_conformance() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        conformance::representative_scenario(
            &conformance::DirectDriver(&connection),
            [f.sessions[0].id.clone(), f.sessions[1].id.clone()],
        )
        .await;
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

pub(super) struct AskPolicy;
impl crate::agent::PreToolPolicy for AskPolicy {
    fn evaluate<'a>(
        &'a self,
        _: &'a crate::agent::PreToolView<'a>,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<crate::agent::PreToolDecision, crate::agent::LifecycleError>,
    > {
        Box::pin(async {
            Ok(crate::agent::PreToolDecision::Ask {
                reason: "protocol regression".into(),
            })
        })
    }
}

async fn call(connection: &AppServerConnection, id: i64, call: Method) -> MethodResult {
    static SCHEMA: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
    let response = connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await;
    let validator = SCHEMA.get_or_init(|| {
        jsonschema::validator_for(&crate::app_server::schema::protocol_schema()).unwrap()
    });
    let wire = serde_json::to_value(&response).unwrap();
    assert!(
        validator.is_valid(&wire),
        "{:?}",
        validator.iter_errors(&wire).collect::<Vec<_>>()
    );
    let Response::Success(response) = response else {
        panic!("{response:?}")
    };
    assert_eq!(response.id, RequestId::Integer(id));
    response.result
}

async fn initialize(connection: &AppServerConnection) {
    call(
        connection,
        0,
        Method::Initialize(InitializeParams {
            protocol_version: crate::app_server::protocol::APP_SERVER_PROTOCOL_VERSION,
            client: ClientIdentity {
                name: "scripted".into(),
                version: "1".into(),
            },
            presentation: PresentationCapabilities::default(),
        }),
    )
    .await;
}

async fn rejected(connection: &AppServerConnection, method: Method) -> ErrorData {
    let Response::Failure(Failure {
        error: RpcError {
            data: Some(data), ..
        },
        ..
    }) = connection
        .handle_request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(999),
            call: method,
        })
        .await
    else {
        panic!("expected typed rejection")
    };
    data
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_unload_reclaims_route_without_notification_polling() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        assert_eq!(connection.attachment_counts(), (1, 0));
        let identity = f.manager.load(&old.session_id, None).await.unwrap();
        identity
            .inspect_runtime()
            .unwrap()
            .fail_residency_settlement();
        assert_eq!(
            rejected(
                &connection,
                Method::SessionUnload {
                    target: old.clone()
                }
            )
            .await,
            ErrorData::OperationFailed
        );
        assert_eq!(
            f.manager.residency(&old.conversation_id),
            super::ResidencyState::Unloading
        );
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(
            rejected(
                &connection,
                Method::TurnStart {
                    target: old,
                    content: (input("never execute"))
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
            ErrorData::StaleAttachment
        );
        let next = attach(&connection, &f, 1).await;
        assert_eq!(connection.attachment_counts(), (1, 0));
        call(&connection, 1, Method::SessionUnload { target: next }).await;
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        // Failed native settlement deliberately retains fail-closed residency
        // until process teardown; connection capacity is independent of it.
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_during_unloading_needs_no_operation_lease() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let probe = f.manager.probe(&old.conversation_id);
        probe.before_shutdown.arm();
        let unload = super::unload_task(&f, old.conversation_id.clone());
        probe.before_shutdown.entered().await;
        assert_eq!(
            f.manager.residency(&old.conversation_id),
            super::ResidencyState::Unloading
        );
        assert!(matches!(
            call(&connection, 1, Method::SessionDetach { target: old }).await,
            MethodResult::Detached {}
        ));
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(!*probe.before_operation.entered.borrow());
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
            Method::SessionSnapshot {
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
async fn admitted_async_operation_drains_before_unload_releases_resources() {
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
            call(
                &worker,
                200,
                Method::ConfigurationGet {
                    target: operation_target,
                },
            )
            .await
        });
        probe.before_operation.entered().await;
        let worker = connection.clone();
        let unload_target = target.clone();
        let unload = tokio::spawn(async move {
            call(
                &worker,
                201,
                Method::SessionUnload {
                    target: unload_target,
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
            super::ResidencyState::Unloading
        );
        assert!(
            !unload.is_finished(),
            "unload is waiting on the admitted operation, not the client"
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
        assert!(matches!(
            operation.await.unwrap(),
            MethodResult::EffectiveConfiguration { .. }
        ));
        assert!(matches!(
            unload.await.unwrap(),
            MethodResult::Unloaded { .. }
        ));
        assert!(weak_host.upgrade().is_none());
        assert!(weak_runtime.upgrade().is_none());
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        assert!(
            matches!(
                f.manager.sessions.delete_preview(&target.session_id).await,
                SessionDeleteResult::Preview { .. }
            ),
            "allocation authority released with all passive client state retained"
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unload_claim_rejects_late_operations_and_old_incarnations() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let old = attach(&connection, &f, 0).await;
        let probe = f.manager.probe(&old.conversation_id);
        probe.before_shutdown.arm();
        let unload = super::unload_task(&f, old.conversation_id.clone());
        probe.before_shutdown.entered().await;
        for _ in 0..2 {
            assert_eq!(
                rejected(
                    &connection,
                    Method::TurnStart {
                        target: old.clone(),
                        content: (input("never execute"))
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
        }
        assert!(
            !*probe.before_operation.entered.borrow(),
            "late operation never admitted"
        );
        assert!(f.provider.request_bodies().is_empty());
        probe.before_shutdown.release();
        unload.await.unwrap().unwrap();
        let replacement = f.manager.load(&old.session_id, None).await.unwrap();
        assert_ne!(old.runtime_incarnation, replacement.incarnation_id());
        assert_eq!(
            rejected(&connection, Method::ConfigurationReload { target: old }).await,
            ErrorData::StaleRuntime
        );
        assert!(f.provider.request_bodies().is_empty());
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

async fn attach_session(
    connection: &AppServerConnection,
    session: &crate::local_runtime::session::SessionSnapshot,
) -> AttachmentTarget {
    let MethodResult::Attached { target, .. } = call(
        connection,
        300,
        Method::SessionAttach {
            session_id: session.id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("attached")
    };
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capacity_rejection_never_composes_and_unload_reclaims_without_notifications() {
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
            super::ResidencyState::Unloaded
        );
        assert_eq!(connection.attachment_counts(), (2, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 2);
        for target in targets.drain(..1) {
            call(&connection, 301, Method::SessionUnload { target }).await;
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
            call(&connection, 302, Method::SessionUnload { target }).await;
        }
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(f.host.diagnostics().external_attachments, 0);
        for session in &sessions {
            assert_eq!(
                f.manager.residency(&session.active_conversation_id),
                super::ResidencyState::Unloaded
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
        for session in &sessions[1..] {
            let worker = connection.clone();
            let barrier = barrier.clone();
            let session_id = session.id.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                worker
                    .handle_request(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(400),
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
            super::ResidencyState::Unloaded
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
            let super::Entry::Loading(flight) = registry.entries.get(&id).unwrap() else {
                panic!("manager-owned Loading flight")
            };
            flight.clone()
        };
        probe.before_compose.release();
        let resident = flight.wait().await.unwrap().unwrap();
        assert_eq!(
            f.manager.residency(resident.conversation_id()),
            super::ResidencyState::Loaded
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
        let unload = super::unload_task(&f, target.conversation_id.clone());
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

async fn attach(connection: &AppServerConnection, f: &Fixture, index: usize) -> AttachmentTarget {
    let MethodResult::Attached { target, .. } = call(
        connection,
        10 + i64::try_from(index).unwrap(),
        Method::SessionAttach {
            session_id: f.sessions[index].id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("attached")
    };
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn initialize_and_malformed_wire_are_transactional() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        let before = connection.handle_json(r#"{"jsonrpc":"2.0","id":0,"method":"server/info","params":{}}"#).await.unwrap();
        assert!(matches!(before, Response::Failure(Failure { error: RpcError { data: Some(ErrorData::NotInitialized), .. }, .. })));
        let bad_version = connection.handle_json(r#"{"jsonrpc":"2.0","id":"version","method":"initialize","params":{"protocol_version":2,"client":{"name":"test","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#).await.unwrap();
        let Response::Failure(failure) = bad_version else { panic!("version mismatch") };
        assert_eq!(failure.id, Some(RequestId::String("version".into())));
        assert!(matches!(failure.error.data, Some(ErrorData::UnsupportedVersion { supported: 6, .. })));
        initialize(&connection).await;
        for (json, expected_code) in [
            (r#"{"jsonrpc":"2.0","id":1,"method":"missing","params":{}}"#, -32601),
            (r#"{"jsonrpc":"2.0","id":2,"method":"session/create","params":{"settings":{}}}"#, -32602),
            (r#"{"jsonrpc":"1.0","id":3,"method":"session/create","params":{}}"#, -32600),
            (r#"{"jsonrpc":"2.0","id":4,"method":"session/create","params":{},"extra":true}"#, -32600),
            ("{", -32700),
            (r#"{"jsonrpc":"2.0","id":9007199254740993,"method":"session/create","params":{}}"#, -32600),
            (r#"{"jsonrpc":"2.0","id":9,"method":"session/list","params":{"offset":9007199254740993,"limit":32}}"#, -32602),
            (r#"{"jsonrpc":"2.0","id":8,"method":"session/name","params":{"session_id":"s","name":"first","name":"second"}}"#, -32602),
            (r#"{"jsonrpc":"2.0","id":6,"method":"settings/saveDefault","params":{"target":{"session_id":"s","conversation_id":"c","runtime_incarnation":1,"attachment_id":"a"},"scope":"user","expected_revision":"r","setting":"settings/saveDefault"}}"#, -32601),
            (r#"{"jsonrpc":"2.0","id":7,"method":"turn/start","params":{"target":{"session_id":"s","conversation_id":"c","runtime_incarnation":1,"attachment_id":"a"},"content":[{"type":"text","text":"never","extra":true}]}}"#, -32602),
        ] {
            let Response::Failure(failure) = connection.handle_json(json).await.unwrap() else { panic!("invalid request accepted") };
            assert_eq!(failure.error.code, expected_code);
            serde_json::to_value(Response::Failure(failure)).expect("even invalid correlation IDs produce a serializable error");
        }
        assert!(connection.handle_json(r#"{"jsonrpc":"2.0","method":"session/create","params":{}}"#).await.is_none());
        let MethodResult::Sessions { sessions, .. } = call(&connection, 5, Method::SessionList { query: None, offset: 0, limit: 32 }).await else { panic!("list") };
        assert_eq!(sessions.len(), 2);
        assert_eq!(f.manager.probe(&f.id(0).await).compositions.load(std::sync::atomic::Ordering::SeqCst), 0);
        f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_session_operations_never_compose_a_runtime() {
    bounded(async {
        use crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult as Deletion;
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let settings = crate::local_runtime::session::SessionPersistentState::from_input(
            &crate::local_runtime::configuration::SessionConfigInput::new(f.workspaces[0].clone()),
        );
        let MethodResult::SessionTransition { session, .. } =
            call(&connection, 100, Method::SessionCreate { settings }).await
        else {
            panic!("create");
        };
        let id = session.id;
        let MethodResult::Sessions {
            sessions,
            residencies,
            ..
        } = call(
            &connection,
            110,
            Method::SessionList {
                query: None,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("list");
        };
        assert_eq!(
            sessions.iter().find(|row| row.id == id).unwrap().cwd,
            f.workspaces[0]
        );
        assert_eq!(residencies.get(&id), Some(&super::ResidencyState::Unloaded));
        let residency_before = f.manager.diagnostics();
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            111,
            Method::SessionSummary {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("exact summary")
        };
        assert_eq!(&summary, sessions.iter().find(|row| row.id == id).unwrap());
        assert_eq!(f.manager.diagnostics(), residency_before);
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        let MethodResult::Session { session: unchanged } = call(
            &connection,
            112,
            Method::SessionRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("read")
        };
        assert_eq!(unchanged.active_node, summary.active_node);
        assert_eq!(unchanged.updated_at, summary.updated_at);
        let missing = crate::local_runtime::session::SessionId::new(
            "ses_00000000-0000-7000-8000-000000000099",
        );
        let absent = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(113),
                call: Method::SessionSummary {
                    session_id: missing.clone(),
                },
            })
            .await;
        assert!(matches!(absent, Response::Failure(Failure {
            error: RpcError { data: Some(ErrorData::UnknownSession { session_id }), .. }, ..
        }) if session_id == missing));
        assert_eq!(f.manager.diagnostics(), residency_before);
        let MethodResult::Session { session } = call(
            &connection,
            101,
            Method::SessionName {
                session_id: id.clone(),
                name: "durable only".into(),
            },
        )
        .await
        else {
            panic!("name");
        };
        assert_eq!(session.name.as_deref(), Some("durable only"));
        let MethodResult::Settings {
            revision,
            mut settings,
            ..
        } = call(
            &connection,
            102,
            Method::SettingsRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("settings");
        };
        settings.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        ));
        let MethodResult::SettingsReplaced { revision: updated } = call(
            &connection,
            103,
            Method::SettingsReplace {
                session_id: id.clone(),
                expected_revision: revision,
                settings: settings.clone(),
            },
        )
        .await
        else {
            panic!("replace");
        };
        assert!(updated > revision);
        let stale = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(104),
                call: Method::SettingsReplace {
                    session_id: id.clone(),
                    expected_revision: revision,
                    settings,
                },
            })
            .await;
        assert!(matches!(
            stale,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::StaleSettings { .. }),
                    ..
                },
                ..
            })
        ));
        let MethodResult::Tree { nodes, .. } = call(
            &connection,
            105,
            Method::SessionTree {
                session_id: id.clone(),
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("tree");
        };
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            f.manager
                .probe(&session.active_conversation_id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let MethodResult::Deletion {
            result: Deletion::Preview { preview },
        } = call(
            &connection,
            106,
            Method::SessionDeletePreview {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("preview");
        };
        assert!(matches!(
            call(
                &connection,
                107,
                Method::SessionDelete {
                    session_id: id.clone(),
                    expected_target_revision: preview.target_revision
                }
            )
            .await,
            MethodResult::Deletion {
                result: Deletion::Deleted { .. }
            }
        ));
        let absent = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(108),
                call: Method::SessionRead { session_id: id },
            })
            .await;
        assert!(matches!(
            absent,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::UnknownSession { .. }),
                    ..
                },
                ..
            })
        ));
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
        let before = call(
            &connection,
            22,
            Method::SessionSnapshot {
                target: a.clone(),
                trace_records: Vec::new(),
            },
        )
        .await;
        let residency_before = f.manager.diagnostics();
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
            Method::SessionSnapshot {
                target: a.clone(),
                trace_records: Vec::new(),
            },
        )
        .await;
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
        let mut seen = std::collections::BTreeSet::new();
        while seen.len() != 2 {
            let NotificationMethod::Event { target, event, .. } =
                connection.next_notification().await.notification
            else {
                panic!("event")
            };
            assert!(target == a || target == b);
            if matches!(*event, RuntimeClientEvent::AttemptStarted { .. }) {
                seen.insert(target.session_id);
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
        let MethodResult::Snapshot { snapshot, .. } = call(
            &rival,
            32,
            Method::SessionSnapshot {
                trace_records: vec![],
                target: new_a,
            },
        )
        .await
        else {
            panic!("snapshot")
        };
        assert_eq!(snapshot.conversation_id, a.conversation_id);
        f.gates[1].release();
        loop {
            if let NotificationMethod::Event { target, event, .. } =
                connection.next_notification().await.notification
                && matches!(*event, RuntimeClientEvent::AttemptSettled { .. })
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
async fn headless_approval_and_questionnaire_survive_detach_and_settle_once() {
    bounded(async {
        use crate::runtime::interaction::{
            ApprovalDecision, InteractionResponse, InteractionSettleGate, QuestionnaireResponse,
        };
        use std::sync::Arc;
        for tool in ["read", "ask_user"] {
            for cancel_first in [false, true] {
                let f = Fixture::with_tool(Some(tool)).await;
                let connection = Arc::new(AppServerConnection::new(f.host.clone()));
                initialize(&connection).await;
                let initial = attach(&connection, &f, 0).await;
                let identity = f.manager.load(&initial.session_id, None).await.unwrap();
                let runtime = identity.inspect_runtime().unwrap();
                if tool == "read" {
                    runtime.install_test_pre_tool_policy(Arc::new(AskPolicy));
                }
                let coordinator = runtime.interaction_test_owner();
                call(
                    &connection,
                    60,
                    Method::TurnStart {
                        target: initial.clone(),
                        content: (input("request-A"))
                            .into_iter()
                            .map(|block| match block {
                                crate::message::types::UserContentBlock::Text(text) => {
                                    crate::app_server::protocol::UserInputBlock::Text(text)
                                }
                                _ => panic!("client fixtures must use text or issued receipts"),
                            })
                            .collect(),
                    },
                )
                .await;
                f.gates[0].wait_entered().await;
                call(&connection, 61, Method::SessionDetach { target: initial }).await;
                // No external attachment exists when the real provider releases the tool call.
                f.gates[0].release();
                coordinator.pending_published.notified().await;
                assert_eq!(coordinator.pending_count(), 1);
                let reattached = attach(&connection, &f, 0).await;
                let MethodResult::Snapshot { snapshot, .. } = call(
                    &connection,
                    62,
                    Method::SessionSnapshot {
                        trace_records: vec![],
                        target: reattached.clone(),
                    },
                )
                .await
                else {
                    panic!("snapshot")
                };
                assert_eq!(snapshot.pending_interactions.len(), 1);
                let pending = snapshot.pending_interactions[0].clone();
                call(
                    &connection,
                    63,
                    Method::SessionDetach { target: reattached },
                )
                .await;
                assert_eq!(
                    coordinator.pending_count(),
                    1,
                    "detach cannot settle a pending interaction"
                );
                let target = attach(&connection, &f, 0).await;
                let response = if tool == "read" {
                    InteractionResponse::Approval {
                        decision: ApprovalDecision::Allow,
                    }
                } else {
                    InteractionResponse::Questionnaire {
                        response: QuestionnaireResponse::Declined,
                    }
                };
                let interaction = pending.interaction.clone();
                let respond = Method::InteractionRespond {
                    target: target.clone(),
                    interaction: interaction.clone(),
                    response,
                };
                let cancel = Method::InteractionCancel {
                    target,
                    interaction,
                };
                let (first, second) = if cancel_first {
                    (cancel, respond)
                } else {
                    (respond, cancel)
                };
                let gate = Arc::new(InteractionSettleGate::default());
                gate.arm();
                coordinator.install_settle_gate(gate.clone());
                let first_connection = connection.clone();
                let winner = tokio::spawn(async move {
                    first_connection
                        .handle_request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(64),
                            call: first,
                        })
                        .await
                });
                let entered = gate.clone();
                tokio::task::spawn_blocking(move || entered.wait_entered())
                    .await
                    .unwrap();
                // The actual coordinator terminal transition is now parked with its winner selected.
                let second_connection = connection.clone();
                let (started, receiver) = tokio::sync::oneshot::channel();
                let loser = tokio::spawn(async move {
                    started.send(()).unwrap();
                    second_connection
                        .handle_request(Request {
                            jsonrpc: JsonRpcVersion::V2,
                            id: RequestId::Integer(65),
                            call: second,
                        })
                        .await
                });
                receiver.await.unwrap();
                // The loser reaches the coordinator while the winning terminal
                // transition is still parked, not merely after task completion.
                let losing_response = loser.await.unwrap();
                gate.release();
                assert!(matches!(winner.await.unwrap(), Response::Success(_)));
                assert!(matches!(losing_response, Response::Failure(_)));
                assert_eq!(coordinator.pending_count(), 0);
                loop {
                    if let NotificationMethod::Event { event, .. } =
                        connection.next_notification().await.notification
                        && matches!(*event, RuntimeClientEvent::AttemptSettled { .. })
                    {
                        break;
                    }
                }
                drop(coordinator);
                drop(runtime);
                drop(connection);
                f.close().await;
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn attach_snapshot_and_subscription_share_the_publication_cut() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let identity = f.manager.load(&f.sessions[0].id, None).await.unwrap();
        let runtime = identity.inspect_runtime().unwrap();
        let probe = crate::runtime_client::test_sync::ProjectionProbe::default();
        {
            let resident = identity.resident.upgrade().unwrap();
            let composition = resident.composition.lock().unwrap();
            composition
                .as_ref()
                .unwrap()
                .host()
                .install_projection_probe(probe.clone());
        }
        probe.arm_snapshot();
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let worker = connection.clone();
        let session_id = f.sessions[0].id.clone();
        let attaching = tokio::spawn(async move {
            call(
                &worker,
                80,
                Method::SessionAttach {
                    session_id,
                    node_id: None,
                },
            )
            .await
        });
        let entered = probe.clone();
        tokio::task::spawn_blocking(move || entered.wait_snapshot_entered())
            .await
            .unwrap();
        // Native producer publication occurs while attach holds the projection cut.
        // No snapshot-then-subscribe window is available to the connection.
        let accepted = runtime.submit_inbound(input("request-A")).unwrap();
        f.gates[0].wait_entered().await;
        probe.release_snapshot();
        let MethodResult::Attached {
            target,
            snapshot,
            cursor,
        } = attaching.await.unwrap()
        else {
            panic!("attach")
        };
        assert!(snapshot.inbound.pending.is_empty());
        let mut previous = cursor.get();
        let mut occurrences = 0;
        f.gates[0].release();
        loop {
            let NotificationMethod::Event {
                target: routed,
                cursor,
                event,
            } = connection.next_notification().await.notification
            else {
                panic!("event")
            };
            assert_eq!(routed, target);
            assert_eq!(cursor.get(), previous + 1);
            previous = cursor.get();
            if let RuntimeClientEvent::InboundEnqueued { message, .. } = &*event {
                assert_eq!(message.id, accepted.message_id);
                occurrences += 1;
            }
            if matches!(*event, RuntimeClientEvent::AttemptSettled { .. }) {
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
            Method::SessionUnload {
                target: target.clone(),
            },
        )
        .await;
        let replacement = f.manager.load(&target.session_id, None).await.unwrap();
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
            Method::SessionSnapshot {
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
async fn admitted_mutation_survives_close_before_native_dispatch() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let worker = connection.clone();
        let pending =
            tokio::spawn(
                async move { call(&worker, 10, Method::ConfigurationReload { target }).await },
            );
        probe.before_operation.entered().await; // manager lease acquired, native call not executed
        connection.close();
        assert_eq!(connection.attachment_counts(), (0, 0));
        let replacement = AppServerConnection::new(f.host.clone());
        initialize(&replacement).await;
        let new = attach(&replacement, &f, 0).await;
        probe.before_operation.release();
        assert!(matches!(
            pending.await.unwrap(),
            MethodResult::ConfigurationReloaded { .. }
        ));
        let MethodResult::Snapshot { snapshot, .. } = call(
            &replacement,
            11,
            Method::SessionSnapshot {
                trace_records: vec![],
                target: new,
            },
        )
        .await
        else {
            panic!("snapshot");
        };
        assert_eq!(snapshot.resources.revision.get(), 2);
        assert!(matches!(
            rejected(&connection, Method::ServerInfo {}).await,
            ErrorData::StaleAttachment
        ));
        replacement.close();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn host_request_owner_outlives_dropped_protocol_waiter() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = std::sync::Arc::new(AppServerConnection::new(f.host.clone()));
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let worker = connection.clone();
        let waiter = tokio::spawn(async move {
            call(
                &worker,
                91,
                Method::SessionSnapshot {
                    trace_records: vec![],
                    target,
                },
            )
            .await
        });
        probe.before_operation.entered().await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            f.host
                .forced_resources(false)
                .contains("pending_protocol_operations=1")
        );
        f.host.begin_drain();
        assert_eq!(
            rejected(&connection, Method::ServerInfo {}).await,
            ErrorData::ServerDraining
        );
        let host = f.host.clone();
        let mut drain = Box::pin(host.drain());
        assert!(futures_util::poll!(&mut drain).is_pending());
        assert!(f.host.finish_drain().is_err());
        probe.before_operation.release();
        assert!(drain.await.is_empty());
        assert!(
            f.host
                .forced_resources(false)
                .contains("pending_protocol_operations=0")
        );
        connection.close();
        f.host.finish_drain().unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artifact_carrier_is_native_scoped_bounded_and_cold_reopen_safe() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let other = attach(&connection, &f, 1).await;
        let artifact_id = {
            let managed = f.load(0).await.unwrap().unwrap();
            managed
                .inspect_runtime()
                .unwrap()
                .tool_runtime()
                .artifacts()
                .put_bounded(b"canonical bytes")
                .unwrap()
        };
        let read = call(
            &connection,
            801,
            Method::ArtifactRead {
                target: target.clone(),
                artifact_id: artifact_id.clone(),
            },
        )
        .await;
        assert_eq!(
            read,
            MethodResult::ArtifactBytes {
                data: "Y2Fub25pY2FsIGJ5dGVz".into()
            }
        );
        let encoded = serde_json::to_string(&read).unwrap();
        assert!(!encoded.contains(f.workspaces[0].parent().unwrap().to_str().unwrap()));
        rejected(
            &connection,
            Method::ArtifactRead {
                target: other,
                artifact_id: artifact_id.clone(),
            },
        )
        .await;
        rejected(
            &connection,
            Method::ArtifactRead {
                target: target.clone(),
                artifact_id: crate::runtime::identity::ArtifactId::new("../conversation.sqlite"),
            },
        )
        .await;
        let uploaded = call(
            &connection,
            910,
            Method::SessionUpload {
                target: target.clone(),
                files: vec![UploadBytes {
                    name: "hello.txt".into(),
                    data: "aGk=".into(),
                }],
            },
        )
        .await;
        assert!(matches!(uploaded, MethodResult::SessionUploaded { .. }));
        for data in [
            "not base64!".to_owned(),
            "A".repeat(crate::tools::artifacts::ARTIFACT_TRANSFER_MAX.div_ceil(3) * 4 + 1),
        ] {
            rejected(
                &connection,
                Method::SessionUpload {
                    target: target.clone(),
                    files: vec![UploadBytes {
                        name: "hello.txt".into(),
                        data,
                    }],
                },
            )
            .await;
        }
        assert!(
            f.provider.request_bodies().is_empty(),
            "storage-only upload never reaches provider"
        );
        call(
            &connection,
            802,
            Method::SessionUnload {
                target: target.clone(),
            },
        )
        .await;
        rejected(
            &connection,
            Method::ArtifactRead {
                target,
                artifact_id: artifact_id.clone(),
            },
        )
        .await;
        let reopened = attach(&connection, &f, 0).await;
        assert_eq!(
            call(
                &connection,
                803,
                Method::ArtifactRead {
                    target: reopened,
                    artifact_id: artifact_id.clone()
                }
            )
            .await,
            read
        );
        let next = {
            let managed = f.load(0).await.unwrap().unwrap();
            managed
                .inspect_runtime()
                .unwrap()
                .tool_runtime()
                .artifacts()
                .put_bounded(b"new bytes")
                .unwrap()
        };
        assert_ne!(artifact_id, next);
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_reads_are_read_only_and_reconnect_repairs_the_same_native_facts() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        call(&connection, 900, Method::TurnStart { target: target.clone(), content: (input("request-A")).into_iter().map(|block| match block {
                    crate::message::types::UserContentBlock::Text(text) => crate::app_server::protocol::UserInputBlock::Text(text),
                    _ => panic!("client fixtures must use text or issued receipts"),
                }).collect()}).await;
        f.gates[0].wait_entered().await;
        let before = call(&connection, 901, Method::SessionSnapshot { trace_records: vec![], target: target.clone() }).await;
        let read = call(&connection, 902, Method::Trace { target: target.clone(), before: None, limit: 32 }).await;
        let MethodResult::Trace { page } = read else { panic!("Trace page"); };
        assert!(page.entries.iter().any(|entry| entry.kind == crate::runtime_client::trace::TraceKind::Request));
        assert!(matches!(rejected(&connection, Method::Trace { target: target.clone(), before: None, limit: 0 }).await, ErrorData::InvalidParams));
        let after = call(&connection, 903, Method::SessionSnapshot { trace_records: vec![], target: target.clone() }).await;
        assert_eq!(before, after, "Trace reads change no live cursor, inbound, interactions, attempt, surface or transcript");
        connection.close();
        let repaired = AppServerConnection::new(f.host.clone());
        initialize(&repaired).await;
        let repaired_target = attach(&repaired, &f, 0).await;
        let MethodResult::Snapshot { snapshot: continuous, .. } = after else { panic!("snapshot"); };
        let MethodResult::Snapshot { snapshot: reconnected, .. } = call(&repaired, 904, Method::SessionSnapshot { trace_records: vec![], target: repaired_target.clone() }).await else { panic!("snapshot"); };
        assert_eq!(continuous.trace, reconnected.trace);
        f.gates[0].release();
        loop {
            if let NotificationMethod::Event { event, .. } = repaired.next_notification().await.notification
                && matches!(*event, RuntimeClientEvent::AttemptSettled { .. }) { break; }
        }
        let MethodResult::Snapshot { snapshot: settled, .. } = call(&repaired, 905, Method::SessionSnapshot { trace_records: vec![], target: repaired_target }).await else { panic!("snapshot"); };
        repaired.close();
        let final_connection = AppServerConnection::new(f.host.clone());
        initialize(&final_connection).await;
        let final_target = attach(&final_connection, &f, 0).await;
        let MethodResult::Snapshot { snapshot: final_snapshot, .. } = call(&final_connection, 906, Method::SessionSnapshot { trace_records: vec![], target: final_target }).await else { panic!("snapshot"); };
        assert_eq!(settled.trace, final_snapshot.trace);
        final_connection.close();
        f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workspace_upload_receipt_admission_and_model_projection_use_one_owner() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let other = attach(&connection, &f, 1).await;
        let MethodResult::SessionUploaded { files } = call(&connection, 1001, Method::SessionUpload {
            target: target.clone(), files: vec![UploadBytes { name: "picture.png".into(), data: "Tk9UX0VBR0VSTFlfSU5KRUNURUQ=".into() }],
        }).await else { panic!("committed upload"); };
        let native = f.load(0).await.unwrap().unwrap().inspect_runtime().unwrap();
        assert!(native.tool_runtime().durable_store().load_canonical().unwrap().is_empty());
        rejected(&connection, Method::TurnStart { target: other, content: vec![UserInputBlock::Upload(files[0].receipt.clone())] }).await;
        assert!(native.tool_runtime().durable_store().load_canonical().unwrap().is_empty());
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"NOT_EAGERLY_INJECTED");
        let body = "request-A\n  exact body  \n";
        call(&connection, 1002, Method::TurnStart { target: target.clone(), content: vec![UserInputBlock::Upload(files[0].receipt.clone()), UserInputBlock::Text(crate::message::content::TextBlock { text: body.into() })] }).await;
        f.gates[0].wait_entered().await;
        let requests = f.provider.request_bodies();
        assert_eq!(requests.len(), 1, "a text-only provider accepts a workspace image");
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        let user = request["messages"].as_array().unwrap().iter().find(|message| message["content"][0]["text"].as_str().is_some_and(|s| s.contains("user_uploaded_files"))).unwrap();
        assert_eq!(user["content"][0]["text"], format!("<user_uploaded_files>\n  <file name=\"picture.png\" path=\"{}\" />\n</user_uploaded_files>\n\n{body}", files[0].path));
        assert!(!requests[0].contains("NOT_EAGERLY_INJECTED"));
        let snapshots = native.request_history().page(None, 4).unwrap().snapshots;
        assert_eq!(snapshots[0].upload_projection.files[0].path, files[0].path);
        let history = native.tool_runtime().durable_store().load_canonical().unwrap();
        let canonical = serde_json::to_string(&history).unwrap();
        assert!(canonical.contains("uploaded_file"));
        assert!(!canonical.contains(&files[0].path));
        assert!(!canonical.contains("artifact_id"));
        // Replay must not consult current ownership metadata, even if it changes.
        let controller = f.manager.session_controller();
        let mut registry = controller.catalog.lock().await.upload_registry(&target.session_id).unwrap();
        registry.allocations.get_mut(&files[0].file.batch_id).unwrap().workspace = f.workspaces[1].clone();
        controller.catalog.lock().await.commit_uploads(&target.session_id, registry).unwrap();
        let reconstructed = snapshots[0].reconstruct_from_canonical(&history).unwrap();
        let frozen = serde_json::to_string(&reconstructed.messages).unwrap();
        assert!(frozen.contains(&files[0].path));
        assert!(!frozen.contains(f.workspaces[1].to_str().unwrap()));
        f.gates[0].release();
        connection.close(); f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lost_upload_waiter_does_not_cancel_or_replay_the_owned_commit() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let controller = f.manager.session_controller();
        let gate = std::sync::Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let release = gate.arm_scoped();
        *controller.upload_commit_gate.lock().unwrap() = Some(gate.clone());
        let caller = connection.clone();
        let waiter = tokio::spawn(async move {
            call(
                &caller,
                1010,
                Method::SessionUpload {
                    target,
                    files: vec![UploadBytes {
                        name: "lost.txt".into(),
                        data: "aGk=".into(),
                    }],
                },
            )
            .await
        });
        tokio::task::spawn_blocking(move || gate.wait_entered())
            .await
            .unwrap();
        waiter.abort();
        let _ = waiter.await;
        connection.close();
        *controller.upload_commit_gate.lock().unwrap() = None;
        drop(release);
        // The next allocation acquires the same preparation owner after the
        // abandoned protocol waiter has left; no scheduler timing is evidence.
        controller
            .upload(
                &f.sessions[0].id,
                None,
                vec![crate::local_runtime::session::uploads::UploadFile {
                    name: "fence.txt".into(),
                    bytes: vec![],
                }],
            )
            .await
            .unwrap();
        let registry = controller
            .catalog
            .lock()
            .await
            .upload_registry(&f.sessions[0].id)
            .unwrap();
        assert_eq!(registry.allocations.len(), 2);
        assert!(registry.allocations.values().all(|a| a.ready));
        assert_eq!(
            registry
                .allocations
                .values()
                .filter(|a| a.files[0].name == "lost.txt")
                .count(),
            1
        );
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_destination_receipts_admit_a_turn_after_source_deletion() {
    bounded(async {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::local_runtime::session::uploads::UploadFile;
        use crate::message::types::{
            InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
        };
        let f = Fixture::new().await;
        let controller = f.manager.session_controller();
        let source = &f.sessions[0];
        let files = controller
            .upload(
                &source.id,
                None,
                vec![
                    UploadFile {
                        name: "A.txt".into(),
                        bytes: b"A".to_vec(),
                    },
                    UploadFile {
                        name: "B.txt".into(),
                        bytes: b"B".to_vec(),
                    },
                ],
            )
            .await
            .unwrap();
        let body = "request-A\n preserve body  \n";
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(
            source.active_conversation_id.clone(),
            &access.database_path,
        )
        .unwrap();
        store
            .append_canonical(&MessageBlock::User(UserMessageBlock {
                id: crate::runtime::identity::MessageId::new("boundary"),
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
                content: vec![
                    UserContentBlock::UploadedFile(files[0].file.clone()),
                    UserContentBlock::Text(crate::message::content::TextBlock {
                        text: body.into(),
                    }),
                    UserContentBlock::UploadedFile(files[1].file.clone()),
                ],
            }))
            .unwrap();
        let revision = store.load_head().unwrap().revision;
        drop(store);
        drop(access);
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::SessionTransition {
            session: destination,
            editor_content: Some(editor),
            ..
        } = call(
            &connection,
            1100,
            Method::SessionFork {
                session_id: source.id.clone(),
                node_id: None,
                surface_revision: revision,
                boundary: Some(crate::runtime::identity::MessageId::new("boundary")),
            },
        )
        .await
        else {
            panic!("fork with editor receipts");
        };
        let crate::local_runtime::session::deletion::SessionDeleteResult::Preview { preview } =
            controller.delete_preview(&source.id).await
        else {
            panic!("preview");
        };
        controller
            .delete_session(&source.id, &preview.target_revision)
            .await
            .unwrap();
        let target = attach_session(&connection, &destination).await;
        rejected(
            &connection,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Upload(files[0].receipt.clone())],
            },
        )
        .await;
        let [
            UserInputBlock::Upload(a),
            UserInputBlock::Text(text),
            UserInputBlock::Upload(b),
        ] = &editor[..]
        else {
            panic!("ordered editor");
        };
        assert_eq!(a.session_id, destination.id);
        assert_eq!(b.session_id, destination.id);
        assert_eq!(text.text, body);
        call(
            &connection,
            1101,
            Method::TurnStart {
                target,
                content: editor,
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let request: serde_json::Value =
            serde_json::from_str(&f.provider.request_bodies()[0]).unwrap();
        let input = request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|m| {
                m["content"][0]["text"]
                    .as_str()
                    .filter(|s| s.contains("user_uploaded_files"))
            })
            .unwrap();
        assert!(input.ends_with(body));
        assert!(input.find("A.txt").unwrap() < input.find("B.txt").unwrap());
        assert!(input.contains(&format!("/uploads/{}/", destination.id.as_str())));
        assert!(!input.contains(&format!("/uploads/{}/", source.id.as_str())));
        f.gates[0].release();
        connection.close();
        f.manager
            .unload(&destination.active_conversation_id)
            .await
            .unwrap();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_pending_mutations_are_routed_cas_bound_and_do_not_cancel_attempts() {
    use crate::durable::inbox::{PendingInboundRef, PendingMutationOutcome as Outcome};
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let a = attach(&connection, &f, 0).await;
        let b = attach(&connection, &f, 1).await;
        let content = |text: &str| {
            vec![UserInputBlock::Text(crate::message::content::TextBlock {
                text: text.into(),
            })]
        };
        call(
            &connection,
            700,
            Method::TurnStart {
                target: a.clone(),
                content: content("request-A"),
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let MethodResult::InboundAccepted {
            message_id,
            inbound_sequence,
        } = call(
            &connection,
            701,
            Method::TurnSteer {
                target: a.clone(),
                content: content("pending original"),
            },
        )
        .await
        else {
            panic!("accepted")
        };
        let expected = PendingInboundRef {
            sequence: inbound_sequence,
            message_id,
            revision: 0,
        };
        assert!(matches!(
            call(
                &connection,
                702,
                Method::InboundRemove {
                    target: b,
                    expected: expected.clone()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::NotPending
            }
        ));
        let mut wrong = a.clone();
        wrong.conversation_id = crate::runtime::identity::ConversationId::new(
            "conv_8810ad58-1e59-72bc-8928-b261707a7130",
        );
        assert_eq!(
            rejected(
                &connection,
                Method::InboundRemove {
                    target: wrong,
                    expected: expected.clone()
                }
            )
            .await,
            ErrorData::StaleAttachment
        );
        let mut wrong_incarnation = a.clone();
        wrong_incarnation.runtime_incarnation = serde_json::from_str("0").unwrap();
        let mut wrong_attachment = a.clone();
        wrong_attachment.attachment_id = crate::runtime_client::AttachmentId::new("obsolete");
        let mut wrong_session = a.clone();
        wrong_session.session_id = f.sessions[1].id.clone();
        for target in [wrong_incarnation, wrong_attachment, wrong_session] {
            assert_eq!(
                rejected(
                    &connection,
                    Method::InboundEdit {
                        target,
                        expected: expected.clone(),
                        text: "must not commit".into(),
                    }
                )
                .await,
                ErrorData::StaleAttachment
            );
        }
        assert!(matches!(
            call(
                &connection,
                703,
                Method::InboundEdit {
                    target: a.clone(),
                    expected: expected.clone(),
                    text: "edited pending".into()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::Applied
            }
        ));
        assert!(matches!(
            call(
                &connection,
                704,
                Method::InboundRemove {
                    target: a.clone(),
                    expected: expected.clone()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::Conflict
            }
        ));
        let MethodResult::Snapshot { snapshot, .. } = call(
            &connection,
            705,
            Method::SessionSnapshot {
                target: a.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("snapshot")
        };
        assert_eq!(snapshot.inbound.pending.len(), 1);
        assert_eq!(snapshot.inbound.pending[0].revision, 1);
        assert_eq!(
            snapshot.inbound.pending[0].message.content,
            input("edited pending")
        );
        let expected = PendingInboundRef {
            revision: 1,
            ..expected
        };
        assert!(matches!(
            call(
                &connection,
                706,
                Method::InboundRemove {
                    target: a.clone(),
                    expected: expected.clone()
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::Applied
            }
        ));
        assert!(matches!(
            call(
                &connection,
                707,
                Method::InboundRemove {
                    target: a.clone(),
                    expected
                }
            )
            .await,
            MethodResult::InboundMutation {
                outcome: Outcome::NotPending
            }
        ));
        let MethodResult::Snapshot { snapshot, .. } = call(
            &connection,
            708,
            Method::SessionSnapshot {
                target: a.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("snapshot")
        };
        assert!(snapshot.inbound.pending.is_empty());
        assert!(
            f.manager
                .load(&a.session_id, None)
                .await
                .unwrap()
                .inspect_runtime()
                .unwrap()
                .has_current_attempt()
        );
        assert_eq!(f.provider.request_bodies().len(), 1);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web08_source_and_session_cas_cross_the_real_protocol_boundary() {
    use crate::local_runtime::configuration::settings::{SourceMutation, SourceScope};
    use crate::model::{catalog::ModelRef, session::SessionModelConfig};
    bounded(async {
        let f = Fixture::new().await;
        // Authorized, canonical TOML; only full-runtime Workflow semantics fail.
        std::fs::write(
            f.workspaces[0].join("rustx.toml"),
            "[agent]\nworkflows = ['check', 'check']\n",
        )
        .unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let id = f.sessions[0].id.clone();
        let MethodResult::SourceSettings {
            projection,
            session_revision,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!()
        };
        assert!(projection.user.authored.is_some());
        assert!(projection.workspace.authored.is_some());
        assert!(projection.user.authored.as_ref().unwrap().models.as_ref().unwrap().contains_key("local/b"));
        let expected = projection.user.revision.clone();
        let selection = Some(SessionModelConfig::of(ModelRef::parse("local/b").unwrap()));
        let MethodResult::SourceSettings {
            projection: saved, ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                session_id: id.clone(),
                expected_revision: expected.clone(),
                mutation: SourceMutation::Config { scope: SourceScope::User, mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel {
                    authored: Some(
                        crate::local_runtime::authoring::ModelLayer {
                            model: Some(ModelRef::parse("local/b").unwrap()),
                            ..Default::default()
                        },
                    ),
                } },
            },
        )
        .await
        else {
            panic!()
        };
        assert_ne!(saved.user.revision, expected);
        assert_eq!(
            saved.user.authored.as_ref().unwrap().agent.as_ref().unwrap().model.as_ref().unwrap().model,
            selection.as_ref().map(|s| s.model.clone())
        );
        assert!(matches!(
            rejected(
                &connection,
                Method::SourcesWrite {
                    session_id: id.clone(),
                    expected_revision: expected,
                    mutation: SourceMutation::Config { scope: SourceScope::User, mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel { authored: None } }
                }
            )
            .await,
            ErrorData::SourceConflict {
                scope: SourceScope::User,
                ..
            }
        ));
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            3,
            Method::SourcesRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!()
        };
        assert_eq!(fresh.user, saved.user);
        let MethodResult::SettingsReplaced { revision } = call(
            &connection,
            4,
            Method::SelectModel {
                session_id: id.clone(),
                expected_revision: session_revision,
                selection: selection.clone(),
            },
        )
        .await
        else {
            panic!()
        };
        assert!(revision > session_revision);
        assert!(matches!(
            rejected(
                &connection,
                Method::SelectModel {
                    session_id: id.clone(),
                    expected_revision: session_revision,
                    selection: None
                }
            )
            .await,
            ErrorData::StaleSettings { .. }
        ));
        let MethodResult::SourceSettings {
            session_selection,
            projection,
            ..
        } = call(
            &connection,
            5,
            Method::SourcesRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!()
        };
        assert_eq!(session_selection, selection);
        assert_eq!(projection.loaded, None);
        call(
            &connection,
            6,
            Method::SelectModel {
                session_id: id,
                expected_revision: revision,
                selection: None,
            },
        )
        .await;
        let error = f
            .manager
            .configuration
            .resolve_session(
                &crate::local_runtime::configuration::SessionConfigInput::new(
                    f.workspaces[0].clone(),
                ),
            )
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("duplicate"), "{error}");
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web08_catalog_commit_preserves_admitted_attempt_and_updates_cold_resolution() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    bounded(Box::pin(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let (initial, _, _) = f
            .manager
            .source_settings(&f.sessions[0].id, None)
            .await
            .unwrap();
        let mut model = initial
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.reasoning = Some(crate::model::authoring::Reasoning {
            default_profile: crate::model::catalog::ReasoningProfileId::new("old"),
            profiles: ["old", "new"]
                .into_iter()
                .map(|name| {
                    (
                        crate::model::catalog::ReasoningProfileId::new(name),
                        crate::model::authoring::Profile {
                            enabled: false,
                            request_params: crate::toml_authoring::RequestParamsToml::default(),
                        },
                    )
                })
                .collect(),
        });
        f.manager
            .source_settings(
                &f.sessions[0].id,
                Some((
                    initial.user.revision,
                    SourceMutation::Config {
                        scope: crate::local_runtime::configuration::settings::SourceScope::User,
                        mutation:
                            crate::local_runtime::configuration::settings::ConfigMutation::Model {
                                id: "local/a".into(),
                                authored: Some(model),
                            },
                    },
                )),
            )
            .await
            .unwrap();
        let target = attach(&connection, &f, 0).await;
        call(
            &connection,
            101,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "request-A".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let runtime = f.manager.load(&target.session_id, None).await.unwrap();
        let before = runtime.client().snapshot().unwrap().0;
        let MethodResult::SourceSettings { projection, .. } = call(
            &connection,
            102,
            Method::SourcesRead {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!()
        };
        let mut model = projection
            .user
            .authored
            .as_ref()
            .unwrap()
            .models
            .as_ref()
            .unwrap()["local/a"]
            .clone();
        model.max_output_tokens = 2048;
        model.reasoning.as_mut().unwrap().default_profile =
            crate::model::catalog::ReasoningProfileId::new("new");
        model.request_params = crate::toml_authoring::RequestParamsToml(
            serde_json::from_value(serde_json::json!({"temperature": 0.8})).unwrap(),
        );
        call(
            &connection,
            103,
            Method::SourcesWrite {
                session_id: target.session_id.clone(),
                expected_revision: projection.user.revision,
                mutation: SourceMutation::Config {
                    scope: crate::local_runtime::configuration::settings::SourceScope::User,
                    mutation:
                        crate::local_runtime::configuration::settings::ConfigMutation::Model {
                            id: "local/a".into(),
                            authored: Some(model),
                        },
                },
            },
        )
        .await;
        let after = runtime.client().snapshot().unwrap().0;
        assert_eq!(before.resources, after.resources);
        assert_eq!(before.capabilities, after.capabilities);
        assert_eq!(
            before.attempt.as_ref().unwrap().execution_settings,
            after.attempt.as_ref().unwrap().execution_settings
        );
        assert_eq!(before.model, after.model);
        assert_eq!(
            before.attempt.as_ref().unwrap().model,
            after.attempt.as_ref().unwrap().model
        );
        let prospective = f
            .manager
            .configuration
            .resolve_session(
                &crate::local_runtime::configuration::SessionConfigInput::new(
                    f.workspaces[0].clone(),
                ),
            )
            .unwrap();
        let next = prospective
            .models
            .model(&crate::model::catalog::ModelRef::parse("local/a").unwrap())
            .unwrap();
        let frozen = after.attempt.as_ref().unwrap().model.as_ref().unwrap();
        assert_eq!(next.id.as_str(), "a");
        assert_eq!(next.max_output_tokens, 2048);
        assert_eq!(frozen.primary.max_output_tokens, 4096);
        assert_eq!(
            next.reasoning.as_ref().unwrap().default_profile.as_str(),
            "new"
        );
        assert_eq!(
            frozen.primary.reasoning_profile.as_ref().unwrap().as_str(),
            "old"
        );
        assert_eq!(next.request_params["temperature"], 0.8);
        assert!(!frozen.primary.request_params.contains_key("temperature"));
        let cold = attach(&connection, &f, 1).await;
        let MethodResult::Models { catalog } =
            call(&connection, 104, Method::ModelCatalog { target: cold }).await
        else {
            panic!()
        };
        assert_eq!(
            catalog
                .models
                .iter()
                .find(|m| m.model.to_string() == "local/a")
                .unwrap()
                .max_output_tokens,
            2048
        );
        f.gates[0].release();
        f.close().await;
    }))
    .await;
}

fn source_gate(
    f: &Fixture,
    point: &'static str,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    f.manager.configuration.test_hooks.insert(point, move || {
        entered.send(()).unwrap();
        resume.recv().unwrap();
    });
    (waiting, release)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg3_source_document_wait_releases_catalog_and_rejects_mixed_session_revision() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let (before, revision, _) = f.manager.source_settings(&id, None).await.unwrap();
        let document_lock = crate::local_runtime::settings::lock_document(std::path::Path::new(&before.user.path)).unwrap();
        let (entered, resume) = source_gate(&f, "before_documents");
        let manager = f.manager.clone();
        let read_id = id.clone();
        let read = tokio::spawn(async move { manager.source_settings(&read_id, None).await });
        entered.await.unwrap();
        resume.send(()).unwrap();
        // Source worker is about to wait on this held document lock.
        // Unrelated catalog access and a durable same-Session commit both finish.
        let mut catalog = f.manager.sessions.catalog.lock().await;
        catalog.settings_revision(&f.sessions[1].id).unwrap();
        let (_, mut settings) = catalog.lineage(&id, None).unwrap();
        settings.model = Some(crate::model::session::SessionModelConfig::of(crate::model::catalog::ModelRef::parse("local/b").unwrap()));
        let next = catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        assert!(!read.is_finished(), "held document lock prevents source completion");
        drop(document_lock);
        assert!(matches!(read.await.unwrap(), Err(super::super::SourceSettingsError::Session(crate::local_runtime::session::SessionError::StaleSettings { expected, actual })) if expected == revision && actual == next));
        let (after, current, selected) = f.manager.source_settings(&id, None).await.unwrap();
        assert_eq!(current, next);
        assert_eq!(selected.unwrap().model.to_string(), "local/b");
        assert_eq!(after.user.revision, before.user.revision);
        f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn web08_selection_validation_releases_catalog_and_commits_with_original_cas() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let (_, revision, _) = f.manager.source_settings(&id, None).await.unwrap();
        let (entered, resume) = source_gate(&f, "validation_started");
        let manager = f.manager.clone();
        let select_id = id.clone();
        let save = tokio::spawn(async move { manager.select_model(&select_id, revision, Some(crate::model::session::SessionModelConfig::of(crate::model::catalog::ModelRef::parse("local/b").unwrap()))).await });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        catalog.settings_revision(&f.sessions[1].id).unwrap();
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        let unchanged = settings.model.clone();
        let next = catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(matches!(save.await.unwrap(), Err(super::super::SourceSettingsError::Session(crate::local_runtime::session::SessionError::StaleSettings { expected, actual })) if expected == revision && actual == next));
        let (_, actual, selection) = f.manager.source_settings(&id, None).await.unwrap();
        assert_eq!(actual, next);
        assert_eq!(selection, unchanged);
        f.close().await;
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg3_source_commit_with_changed_session_is_uncertain_and_never_replayed() {
    use crate::local_runtime::configuration::settings::{SettingsError, SourceMutation};
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let (before, revision, _) = f.manager.source_settings(&id, None).await.unwrap();
        let (entered, resume) = source_gate(&f, "before_publication");
        let mut model = before.user.authored.as_ref().unwrap().models.as_ref().unwrap()["local/a"].clone();
        model.max_output_tokens = 2048;
        let manager = f.manager.clone();
        let write_id = id.clone();
        let write = tokio::spawn(async move {
            manager
                .source_settings(
                    &write_id,
                    Some((
                        before.user.revision,
                        SourceMutation::Config { scope: crate::local_runtime::configuration::settings::SourceScope::User, mutation: crate::local_runtime::configuration::settings::ConfigMutation::Model { id: "local/a".into(), authored: Some(model) } },
                    )),
                )
                .await
        });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(matches!(
            write.await.unwrap(),
            Err(super::super::SourceSettingsError::Source(
                SettingsError::Committed
            ))
        ));
        let (fresh, _, _) = f.manager.source_settings(&id, None).await.unwrap();
        assert_eq!(
            fresh.user.authored.as_ref().unwrap().models.as_ref().unwrap()["local/a"].max_output_tokens,
            2048
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg3_mcp_cas_and_committed_uncertainty_use_existing_settings_boundary() {
    use crate::local_runtime::configuration::settings::{
        McpWrite, SettingsError, SourceMutation, SourceScope,
    };
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let (before, revision, _) = f.manager.source_settings(&id, None).await.unwrap();
        let mutation = SourceMutation::Mcp {
            scope: SourceScope::User,
            id: crate::runtime::identity::McpServerId::new("fixture"),
            authored: Some(McpWrite {
                definition: serde_json::from_value(
                    serde_json::json!({"command":"never-start-unselected-fixture"}),
                )
                .unwrap(),
                retained_env: vec![],
                retained_headers: vec![],
            }),
        };
        let (entered, resume) = source_gate(&f, "before_publication");
        let manager = f.manager.clone();
        let write_id = id.clone();
        let expected = before.user_mcp.revision.clone();
        let write = tokio::spawn(async move {
            manager
                .source_settings(&write_id, Some((expected, mutation)))
                .await
        });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(matches!(
            write.await.unwrap(),
            Err(super::super::SourceSettingsError::Source(
                SettingsError::Committed
            ))
        ));
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!()
        };
        assert!(
            fresh
                .user_mcp
                .authored
                .as_ref()
                .unwrap()
                .contains_key(&crate::runtime::identity::McpServerId::new("fixture"))
        );
        assert!(matches!(
            rejected(
                &connection,
                Method::SourcesWrite {
                    session_id: id.clone(),
                    expected_revision: before.user_mcp.revision,
                    mutation: SourceMutation::Mcp {
                        scope: SourceScope::User,
                        id: crate::runtime::identity::McpServerId::new("fixture"),
                        authored: None
                    }
                }
            )
            .await,
            ErrorData::SourceConflict { .. }
        ));
        let MethodResult::SourceSettings {
            projection: deleted,
            ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                session_id: id,
                expected_revision: fresh.user_mcp.revision,
                mutation: SourceMutation::Mcp {
                    scope: SourceScope::User,
                    id: crate::runtime::identity::McpServerId::new("fixture"),
                    authored: None,
                },
            },
        )
        .await
        else {
            panic!()
        };
        assert!(deleted.user_mcp.authored.as_ref().unwrap().is_empty());
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cfg332_workspace_mcp_definition_and_policy_have_independent_authoring_owners() {
    use crate::local_runtime::configuration::settings::{
        ConfigMutation, McpWrite, SourceMutation, SourceScope,
    };
    use crate::runtime::identity::McpServerId;
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let (before, _, _) = f.manager.source_settings(&id, None).await.unwrap();
        let MethodResult::SourceSettings {
            projection: definition,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesWrite {
                session_id: id.clone(),
                expected_revision: before.workspace_mcp.revision,
                mutation: SourceMutation::Mcp {
                    scope: SourceScope::Workspace,
                    id: McpServerId::new("service"),
                    authored: Some(McpWrite {
                        definition: serde_json::from_value(
                            serde_json::json!({"command":"never-start-unselected-fixture"}),
                        )
                        .unwrap(),
                        retained_env: vec![],
                        retained_headers: vec![],
                    }),
                },
            },
        )
        .await
        else {
            panic!("source projection")
        };
        assert_eq!(definition.workspace.revision, before.workspace.revision);
        let MethodResult::SourceSettings {
            projection: policy, ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                session_id: id.clone(),
                expected_revision: definition.workspace.revision,
                mutation: SourceMutation::Config {
                    scope: SourceScope::Workspace,
                    mutation: ConfigMutation::McpPolicy {
                        id: McpServerId::new("service"),
                        authored: Some(crate::local_runtime::config::InvocationPolicyDocument {
                            approval: crate::local_runtime::config::ApprovalPolicyDocument::Always,
                            ..Default::default()
                        }),
                    },
                },
            },
        )
        .await
        else {
            panic!("policy projection")
        };
        assert_eq!(
            policy.workspace_mcp.revision,
            definition.workspace_mcp.revision
        );
        assert_ne!(policy.workspace.revision, before.workspace.revision);
        assert_eq!(policy.user.revision, before.user.revision);
        let target = attach(&connection, &f, 0).await;
        let MethodResult::EffectiveConfiguration { projection } =
            call(&connection, 3, Method::ConfigurationGet { target }).await
        else {
            panic!("native effective projection")
        };
        assert!(projection.root_agent.tools.sources.is_empty());
        assert!(f.provider.request_bodies().is_empty());
        connection.close();
        f.close().await;
    })
    .await;
}
