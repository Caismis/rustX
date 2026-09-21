//! Direct connection contracts. Provider gates prove overlap; no socket timing.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
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
        let MethodResult::Deletion { result: crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult::Preview { preview } } =
            call(&connection, 199, Method::SessionDeletePreview { session_id: target.session_id.clone() }).await else { panic!("preview") };
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
            super::ResidencyState::Unloading
        );
        assert!(
            !delete.is_finished(),
            "unload is waiting on the admitted operation, not the client"
        );
        assert!(matches!(
            call(&connection, 202, Method::SessionRecoverDeletion { session_id: target.session_id.clone() }).await,
            MethodResult::Deletion { result: crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult::Preview { .. } }
        ));
        assert!(f.manager.registry.0.lock().unwrap().retiring_sessions.contains(&target.session_id));
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
            super::ResidencyState::Unloaded
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
                super::ResidencyState::Loaded
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
        let resident = flight.wait().await.operation_result().unwrap().unwrap();
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
        let bad_version = connection.handle_json(r#"{"jsonrpc":"2.0","id":"version","method":"initialize","params":{"protocol_version":12,"client":{"name":"test","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#).await.unwrap();
        let Response::Failure(failure) = bad_version else { panic!("version mismatch") };
        assert_eq!(failure.id, Some(RequestId::String("version".into())));
        assert!(matches!(failure.error.data, Some(ErrorData::UnsupportedVersion { supported: 16, requested: 12 })));
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
        let MethodResult::Sessions { sessions, .. } = call(
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
            revision: _,
            settings,
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
        assert!(
            settings.model.is_some(),
            "Session creation resolves its model"
        );
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
            ..
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
            Method::SessionDetach {
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
        assert!(page.records.iter().any(|entry| entry.kind == crate::runtime_client::trace::TraceKind::Request));
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
                side: crate::local_runtime::session::LineageSide::Before,
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
    use crate::local_runtime::configuration::settings::SourceMutation;
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
        let _id = f.sessions[0].id.clone();
        let MethodResult::SourceSettings {
            projection,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        assert!(projection.user.authored.is_some());
        assert!(projection.workspace.is_none());
        assert!(projection.user.authored.as_ref().unwrap().models.as_ref().unwrap().contains_key("local/b"));
        let expected = projection.user.revision.clone();
        let selection = Some(SessionModelConfig::of(ModelRef::parse("local/b").unwrap()));
        let MethodResult::SourceSettings {
            projection: saved, ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: expected.clone(),
                mutation: SourceMutation::Config { mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel {
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
                    target: crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected_revision: expected,
                    mutation: SourceMutation::Config { mutation: crate::local_runtime::configuration::settings::ConfigMutation::RootModel { authored: None } }
                }
            )
            .await,
            ErrorData::SourceConflict {
                ..
            }
        ));
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            3,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
            },
        )
        .await
        else {
            panic!()
        };
        assert_eq!(fresh.user, saved.user);
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
        let initial = f
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                None,
            )
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
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                Some((
                    initial.user.revision,
                    SourceMutation::Config {
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
        if let Some(candidate) = f
            .manager
            .configuration_application(&target.session_id)
            .and_then(|application| application.candidate)
        {
            f.manager
                .adopt_configuration(
                    &target.session_id,
                    &candidate.identity,
                    candidate.expected_binding,
                )
                .unwrap();
        }
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
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
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
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: projection.user.revision,
                mutation: SourceMutation::Config {
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
        // Creation uses the successfully prepared source authority, after
        // native processing acknowledges availability. Existing A stays frozen.
        super::configuration::settled(&f, 0).await;
        let created = f
            .manager
            .create_session(crate::local_runtime::session::SessionPersistentState {
                cwd: f.workspaces[0].clone(),
                model: None,
            })
            .await
            .unwrap();
        let cold = attach_session(&connection, &created.session).await;
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
async fn c12_source_read_is_independent_of_concurrent_session_revision() {
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let before = f
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                None,
            )
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let document_lock =
            crate::local_runtime::settings::lock_document(std::path::Path::new(&before.user.path))
                .unwrap();
        let (entered, resume) = source_gate(&f, "before_documents");
        let manager = f.manager.clone();
        let _read_id = id.clone();
        let read = tokio::spawn(async move {
            manager
                .source_settings(
                    &crate::local_runtime::configuration::settings::SourceTarget::User,
                    None,
                )
                .await
        });
        entered.await.unwrap();
        resume.send(()).unwrap();
        // Source worker is about to wait on this held document lock.
        // Unrelated catalog access and a durable same-Session commit both finish.
        let mut catalog = f.manager.sessions.catalog.lock().await;
        catalog.settings_revision(&f.sessions[1].id).unwrap();
        let (_, mut settings) = catalog.lineage(&id, None).unwrap();
        settings.model = Some(crate::model::session::SessionModelConfig::of(
            crate::model::catalog::ModelRef::parse("local/b").unwrap(),
        ));
        let next = catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        assert!(
            !read.is_finished(),
            "held document lock prevents source completion"
        );
        drop(document_lock);
        assert!(read.await.unwrap().is_ok());
        let after = f
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                None,
            )
            .await
            .unwrap();
        let (current, settings) = f.manager.sessions.read_settings(&id).await.unwrap();
        let selected = settings.model;
        assert_eq!(current, next);
        assert_eq!(selected.unwrap().model.to_string(), "local/b");
        assert_eq!(after.user.revision, before.user.revision);
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_source_commit_is_independent_of_concurrent_session_revision() {
    use crate::local_runtime::configuration::settings::SourceMutation;
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let before = f.manager.source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User, None).await.unwrap();
        let revision = f.manager.sessions.catalog.lock().await.settings_revision(&id).unwrap();
        let (entered, resume) = source_gate(&f, "before_publication");
        let mut model = before.user.authored.as_ref().unwrap().models.as_ref().unwrap()["local/a"].clone();
        model.max_output_tokens = 2048;
        let manager = f.manager.clone();
        let _write_id = id.clone();
        let write = tokio::spawn(async move {
            manager
                .source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User,
                    Some((
                        before.user.revision,
                        SourceMutation::Config { mutation: crate::local_runtime::configuration::settings::ConfigMutation::Model { id: "local/a".into(), authored: Some(model) } },
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
        assert!(write.await.unwrap().is_ok());
        let fresh = f.manager.source_settings(&crate::local_runtime::configuration::settings::SourceTarget::User, None).await.unwrap();
        assert_eq!(
            fresh.user.authored.as_ref().unwrap().models.as_ref().unwrap()["local/a"].max_output_tokens,
            2048
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c12_mcp_source_commit_is_independent_of_session_revision() {
    use crate::local_runtime::configuration::settings::{McpWrite, SourceMutation};
    bounded(async {
        let f = Fixture::new().await;
        let id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let before = f
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::User,
                None,
            )
            .await
            .unwrap();
        let revision = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .settings_revision(&id)
            .unwrap();
        let mutation = SourceMutation::Mcp {
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
        let _write_id = id.clone();
        let expected = before.user_mcp.revision.clone();
        let write = tokio::spawn(async move {
            manager
                .source_settings(
                    &crate::local_runtime::configuration::settings::SourceTarget::User,
                    Some((expected, mutation)),
                )
                .await
        });
        entered.await.unwrap();
        let mut catalog = f.manager.sessions.catalog.lock().await;
        let (_, settings) = catalog.lineage(&id, None).unwrap();
        catalog.replace_settings(&id, revision, settings).unwrap();
        drop(catalog);
        resume.send(()).unwrap();
        assert!(write.await.unwrap().is_ok());
        let MethodResult::SourceSettings {
            projection: fresh, ..
        } = call(
            &connection,
            1,
            Method::SourcesRead {
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
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
                    target: crate::local_runtime::configuration::settings::SourceTarget::User,
                    expected_revision: before.user_mcp.revision,
                    mutation: SourceMutation::Mcp {
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
                target: crate::local_runtime::configuration::settings::SourceTarget::User,
                expected_revision: fresh.user_mcp.revision,
                mutation: SourceMutation::Mcp {
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
    use crate::local_runtime::configuration::settings::{ConfigMutation, McpWrite, SourceMutation};
    use crate::runtime::identity::McpServerId;
    bounded(async {
        let f = Fixture::new().await;
        let _id = f.sessions[0].id.clone();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let before = f
            .manager
            .source_settings(
                &crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
                None,
            )
            .await
            .unwrap();
        let MethodResult::SourceSettings {
            projection: definition,
            ..
        } = call(
            &connection,
            1,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
                expected_revision: before.workspace_mcp.as_ref().unwrap().revision.clone(),
                mutation: SourceMutation::Mcp {
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
        assert_eq!(
            definition.workspace.as_ref().unwrap().revision,
            before.workspace.as_ref().unwrap().revision
        );
        let MethodResult::SourceSettings {
            projection: policy, ..
        } = call(
            &connection,
            2,
            Method::SourcesWrite {
                target: crate::local_runtime::configuration::settings::SourceTarget::Workspace {
                    directory: f.workspaces[0].clone(),
                },
                expected_revision: definition.workspace.as_ref().unwrap().revision.clone(),
                mutation: SourceMutation::Config {
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
            policy.workspace_mcp.as_ref().unwrap().revision,
            definition.workspace_mcp.as_ref().unwrap().revision
        );
        assert_ne!(
            policy.workspace.as_ref().unwrap().revision,
            before.workspace.as_ref().unwrap().revision
        );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_connection_deletes_an_attached_idle_session_and_closes_its_route() {
    bounded(async {
        use crate::runtime_client::session_deletion::RuntimeClientSessionDeletionResult as Deleted;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // One native source proves both lineage destinations and Retry through the public protocol.
async fn completed_response_cut_is_shared_by_branch_and_fork_and_distinct_from_retry() {
    Box::pin(bounded(async {
        use crate::durable::{ConversationStore, SqliteConversationStore};
        use crate::events::types::{RuntimeEvent, RuntimeEventEnvelope};
        use crate::local_runtime::session::LineageSide;
        use crate::message::types::{AssistantContentBlock, AssistantMessageBlock, InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
        use crate::message::TextBlock;
        use crate::model::finish::ModelFinishReason;
        use crate::runtime::identity::{AttemptId, EventId, MessageId};
        let f = Fixture::new().await;
        let source = &f.sessions[0];
        let controller = f.manager.session_controller();
        let access = controller.acquire_session(&source.id, None).await.unwrap();
        let store = SqliteConversationStore::open(source.active_conversation_id.clone(), &access.database_path).unwrap();
        let event = |attempt: &str, kind| RuntimeEventEnvelope {
            schema_version: 1, event_id: EventId::new(format!("lineage-event-{}", store.presentation_frontier().unwrap() + 1)), sequence: 0,
            conversation_id: source.active_conversation_id.clone(), attempt_id: Some(AttemptId::new(attempt)), turn_id: None,
            timestamp: chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap(), event: kind,
        };
        for (input, output, attempt) in [("user-a", "assistant-a", "a"), ("user-b", "assistant-b", "b")] {
            store.append_canonical(&MessageBlock::User(UserMessageBlock { id: MessageId::new(input), source: UserSource::Human,
                kind: InboundKind::Message, timestamp: None, content: vec![UserContentBlock::Text(TextBlock { text: input.into() })] })).unwrap();
            store.append_event(event(attempt, RuntimeEvent::AttemptStarted { attempt_id: AttemptId::new(attempt) })).unwrap();
            crate::runtime_client::response::tests::request(&store, attempt, 0, Some(crate::model::ModelUsage { input_tokens: 100, output_tokens: 20, total_tokens: 120, details: None }));
            store.append_canonical_with_event(&MessageBlock::Assistant(AssistantMessageBlock { id: MessageId::new(output),
                content: vec![AssistantContentBlock::Text(TextBlock { text: output.into() })] }),
                event(attempt, RuntimeEvent::AssistantMessageCommitted { message_id: MessageId::new(output) })).unwrap();
            store.append_event(event(attempt, RuntimeEvent::AttemptCompleted { attempt_id: AttemptId::new(attempt), finish_reason: ModelFinishReason::Stop })).unwrap();
        }
        let revision = store.message_append_revision(&MessageId::new("assistant-b")).unwrap().unwrap();
        let original = store.load_canonical().unwrap();
        drop(access);
        let connection = AppServerConnection::new(f.host.clone()); initialize(&connection).await;
        for (index, fork) in [false, true].into_iter().enumerate() {
            let method = if fork { Method::SessionFork { session_id: source.id.clone(), node_id: Some(source.active_node.clone()), surface_revision: revision, boundary: Some(MessageId::new("assistant-b")), side: LineageSide::After } }
                else { Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: revision, boundary: MessageId::new("assistant-b"), side: LineageSide::After } };
            let MethodResult::SessionTransition { session, editor_content, .. } = call(&connection, 4000 + i64::try_from(index).unwrap(), method).await else { panic!("lineage transition") };
            assert_eq!(session.id == source.id, !fork);
            assert!(editor_content.is_none_or(|content| content.is_empty()));
            let destination = controller.acquire_session(&session.id, Some(&session.active_node)).await.unwrap();
            let copied = SqliteConversationStore::open_existing(session.active_conversation_id, &destination.database_path).unwrap();
            let messages = copied.load_canonical().unwrap(); assert_eq!(messages.len(), 4);
            assert!(matches!(&messages[3], MessageBlock::Assistant(a) if a.content == match &original[3] { MessageBlock::Assistant(a) => a.content.clone(), _ => unreachable!() }));
            let mut projected = crate::runtime_client::snapshot::transcript_page_view(copied.load_transcript_page(None, 64).unwrap()).unwrap();
            crate::runtime_client::response::decorate(&copied, &mut projected).unwrap();
            let tail = projected.entries.last().unwrap().completed_response.clone().unwrap();
            assert_eq!(tail.closing_message_id, crate::conversation::message_id_of(&messages[3]));
            assert_eq!(tail.retry_message_id, Some(crate::conversation::message_id_of(&messages[2])));
            assert_eq!(tail.origin.conversation_id, source.active_conversation_id);
            assert_eq!(tail.usage.as_ref().unwrap().total_tokens, 120);
            assert_eq!(tail.timing.as_ref().unwrap().generation_ms, Some(1280));
            assert_eq!(projected.entries.iter().filter(|entry| entry.completed_response.is_some()).count(), 2);
            assert_eq!(projected.statistics.unwrap(), crate::runtime_client::response::ConversationStatistics::default());
            assert!(copied.read_events(None, 128).unwrap().events.is_empty());
            let child_id = session.id.clone(); let child_node = session.active_node.clone();
            drop(copied); drop(destination);
            let MethodResult::Attached { target: child_target, snapshot: attached, .. } = call(&connection, 4198, Method::SessionAttach { session_id: child_id.clone(), node_id: Some(child_node.clone()) }).await else { panic!("attach inherited lineage") };
            assert_eq!(attached.transcript.entries.last().unwrap().completed_response.as_ref(), Some(&tail));
            call(&connection, 4199, Method::SessionDetach { target: child_target }).await;
            // Both independent and in-Session children remain native anchors on another copy.
            for again_fork in [false, true] {
                let method = if again_fork { Method::SessionFork { session_id: child_id.clone(), node_id: Some(child_node.clone()), surface_revision: tail.surface_revision, boundary: Some(tail.closing_message_id.clone()), side: LineageSide::After } }
                    else { Method::SessionBranch { session_id: child_id.clone(), node_id: child_node.clone(), surface_revision: tail.surface_revision, boundary: tail.closing_message_id.clone(), side: LineageSide::After } };
                let MethodResult::SessionTransition { editor_content, .. } = call(&connection, 4200, method).await else { panic!("inherited continuation") };
                assert!(editor_content.is_none_or(|content| content.is_empty()));
            }
            let MethodResult::SessionTransition { session: retry, editor_content: Some(input), .. } = call(&connection, 4201, Method::SessionBranch { session_id: child_id.clone(), node_id: child_node.clone(), surface_revision: tail.surface_revision, boundary: tail.retry_message_id.clone().unwrap(), side: LineageSide::Before }).await else { panic!("inherited retry") };
            assert_eq!(input, vec![UserInputBlock::Text(TextBlock { text: "user-b".into() })]);
            let retry_access = controller.acquire_session(&retry.id, Some(&retry.active_node)).await.unwrap();
            let retry_store = SqliteConversationStore::open_existing(retry.active_conversation_id, &retry_access.database_path).unwrap();
            assert_eq!(retry_store.load_canonical().unwrap().len(), 2);
            let child = controller.acquire_session(&child_id, Some(&child_node)).await.unwrap();
            let reopened = SqliteConversationStore::open_existing(child.node.conversation_id.clone(), &child.database_path).unwrap();
            assert_eq!(reopened.load_canonical().unwrap(), messages);
            let mut reopened_page = crate::runtime_client::snapshot::transcript_page_view(reopened.load_transcript_page(None, 64).unwrap()).unwrap();
            crate::runtime_client::response::decorate(&reopened, &mut reopened_page).unwrap();
            assert_eq!(reopened_page.entries.last().unwrap().completed_response.as_ref(), Some(&tail));
            rejected(&connection, Method::SessionBranch { session_id: child_id, node_id: child_node, surface_revision: crate::conversation::SurfaceRevision::new(tail.surface_revision.get() + 1), boundary: tail.closing_message_id, side: LineageSide::After }).await;

        }
        let MethodResult::SessionTransition { session, editor_content: Some(input), .. } = call(&connection, 4010, Method::SessionBranch {
            session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: revision, boundary: MessageId::new("user-b"), side: LineageSide::Before,
        }).await else { panic!("retry cut") };
        assert_eq!(input, vec![UserInputBlock::Text(TextBlock { text: "user-b".into() })]);
        let destination = controller.acquire_session(&session.id, Some(&session.active_node)).await.unwrap();
        let retry = SqliteConversationStore::open_existing(session.active_conversation_id, &destination.database_path).unwrap();
        assert_eq!(retry.load_canonical().unwrap().len(), 2);
        rejected(&connection, Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: crate::conversation::SurfaceRevision::new(revision.get() + 1), boundary: MessageId::new("assistant-b"), side: LineageSide::After }).await;
        rejected(&connection, Method::SessionBranch { session_id: source.id.clone(), node_id: source.active_node.clone(), surface_revision: revision, boundary: MessageId::new("user-b"), side: LineageSide::After }).await;
        assert_eq!(store.load_canonical().unwrap(), original);
        drop(destination); drop(retry); drop(store); f.close().await;
    })).await;
}

#[tokio::test]
async fn archive_preflight_failures_reach_the_protocol_without_private_diagnostics() {
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::runtime::identity::{AgentId, ConversationId, SubagentId, ToolCallId};
    use crate::session_archive::SessionArchivePrepareError;
    bounded(async {
        let f = Fixture::new().await;
        let root = crate::runtime::local_storage::ProductRoot::existing(&f.archive_root).unwrap();
        let catalog = crate::local_runtime::session::SessionCatalog::read_under_guard(&root).unwrap().unwrap();
        let session = &f.sessions[0];
        let store = SqliteConversationStore::open(session.active_conversation_id.clone(), &catalog.database_path(&session.id, &session.active_conversation_id)).unwrap();
        let child = ConversationId::generate();
        let event = crate::runtime::subagent::ownership_event(
            &session.active_conversation_id, &SubagentId::for_conversation(&session.active_conversation_id, 1),
            &AgentId::new("archive-child"), &child, &ToolCallId::new("archive-child-call"),
            &crate::runtime::subagent::SubagentName::parse("explore").unwrap(),
            &serde_json::from_value(serde_json::json!("sha256:definition")).unwrap(),
            &serde_json::from_value(serde_json::json!(format!("sha256:{}", "a".repeat(64)))).unwrap(),
            crate::events::types::SubagentOwnershipKind::Normal,
            &crate::runtime::workspace::WorkspaceSnapshot::shared(f.workspaces[0].clone()), chrono::Utc::now(),
        );
        store.append_event(event).unwrap(); // Required child deliberately has no allocation.
        let session = &f.sessions[1];
        let store = SqliteConversationStore::open(session.active_conversation_id.clone(), &catalog.database_path(&session.id, &session.active_conversation_id)).unwrap();
        let mut message = crate::message::types::UserMessageBlock {
            id: crate::runtime::identity::MessageId::new("archive-artifact"), content: input("authored"),
            source: crate::message::types::UserSource::Human, kind: crate::message::types::InboundKind::default(), timestamp: None,
        };
        message.content.push(crate::message::types::UserContentBlock::File(crate::message::content::FileReference {
            artifact_id: crate::runtime::identity::ArtifactId::new("artifact_1"), name: None, mime_type: None, description: None,
        }));
        store.append_canonical(&crate::message::types::MessageBlock::User(message)).unwrap();
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        for (index, reason) in [SessionArchivePrepareError::DescendantUnavailable, SessionArchivePrepareError::ArtifactUnavailable].into_iter().enumerate() {
            let response = connection.handle_request(Request { jsonrpc: JsonRpcVersion::V2, id: RequestId::Integer(2),
                call: Method::SessionExportPrepare { session_id: f.sessions[index].id.clone() } }).await;
            let Response::Failure(failure) = response else { panic!("preflight returned a descriptor") };
            assert_eq!(failure.error.data, Some(ErrorData::ArchivePreparationFailed { reason }));
            assert_eq!(failure.error.message, reason.to_string());
            assert!(!failure.error.message.contains(f.archive_root.to_str().unwrap()));
            let encoded = serde_json::to_value(Response::Failure(failure.clone())).unwrap();
            assert!(jsonschema::validator_for(&crate::app_server::schema::protocol_schema()).unwrap().is_valid(&encoded));
            // These exact native errors are generated into the fixtures used by both clients.
            assert!(crate::app_server::schema::fixtures().iter().any(|fixture| {
                matches!(fixture, ProtocolMessage::Response(Response::Failure(item)) if item.error == failure.error)
            }));
            if index == 0 {
                // Global ownership is required even when exporting another Session.
                // Repair the missing child before independently exercising artifacts.
                let path = catalog.database_path(&f.sessions[0].id, &child);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                SqliteConversationStore::open(child.clone(), &path).unwrap().initialize(&[]).unwrap();
            }
        }
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert!(f.provider.request_bodies().is_empty());
        f.close().await;
    }).await;
}

fn wire_text(text: &str) -> Vec<UserInputBlock> {
    vec![UserInputBlock::Text(crate::message::content::TextBlock {
        text: text.into(),
    })]
}

/// Drains events until the named Session's current attempt settles: the
/// deterministic "turn completed" signal (a settled attempt provably follows
/// the canonical commit of its user boundary).
async fn await_attempt_settled(
    connection: &AppServerConnection,
    session_id: &crate::local_runtime::session::SessionId,
) {
    loop {
        let NotificationMethod::Event { target, event, .. } =
            connection.next_notification().await.notification
        else {
            panic!("event")
        };
        if target.session_id == *session_id
            && matches!(*event, RuntimeClientEvent::AttemptSettled { .. })
        {
            break;
        }
    }
}

// P03 (app-server plane): catalog searches by identity, name, and persisted
// projection are served from the catalog alone — zero conversation-store
// opens on the serving thread.
#[tokio::test]
async fn session_list_searches_by_identity_name_and_projection_open_no_store() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let session = &f.sessions[0];
        f.manager
            .sessions
            .rename_session(&session.id, "projection search name")
            .await
            .unwrap();
        assert!(
            f.manager
                .sessions
                .publish_display_preview(&session.id, "projection search line")
                .await
                .unwrap()
        );
        for query in [
            session.id.as_str().to_owned(),
            "projection search name".to_owned(),
            "projection search line".to_owned(),
        ] {
            let before = crate::durable::conversation_store_opens_on_this_thread();
            let MethodResult::Sessions { sessions, .. } = call(
                &connection,
                40,
                Method::SessionList {
                    query: Some(query),
                    offset: 0,
                    limit: 32,
                },
            )
            .await
            else {
                panic!("list")
            };
            let after = crate::durable::conversation_store_opens_on_this_thread();
            assert_eq!(
                after, before,
                "a catalog search never opens a conversation store"
            );
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, session.id);
            assert_eq!(sessions[0].name.as_deref(), Some("projection search name"));
            assert_eq!(
                sessions[0].preview.as_deref(),
                Some("projection search line")
            );
        }
        f.close().await;
    })
    .await;
}

// P05 + P06 (runtime plane): the first turn's canonical commit publishes the
// normalized, 120-character-bounded projection while the turn is still
// parked; a later turn never repaints it and never commits the catalog again.
#[tokio::test]
async fn the_first_turn_publishes_the_bounded_projection_and_later_turns_never_repaint() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        // Whitespace-heavy, multiline, multi-script, and long enough that the
        // 119th character is a non-space (so the kept prefix is not trimmed).
        let text = format!("  {}\n\n\t {}", "é".repeat(150), "汉".repeat(50));
        assert!(!text.contains("request-B"));
        let reply = call(
            &connection,
            50,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text(&text),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(probe.attempted && probe.published);
        }
        // The companion rendering: the wire row must equal `preview_of` of
        // the same boundary, normalized to one line and bounded at 120 chars.
        let expected =
            crate::local_runtime::session::preview_of(&crate::message::types::UserMessageBlock {
                id: crate::runtime::identity::MessageId::new("p05-companion"),
                content: input(&text),
                source: crate::message::types::UserSource::Human,
                kind: crate::message::types::InboundKind::Message,
                timestamp: None,
            })
            .expect("text renders a line");
        assert_eq!(expected.chars().count(), 120);
        assert!(expected.ends_with('\u{2026}'));
        let before = crate::durable::conversation_store_opens_on_this_thread();
        let MethodResult::Sessions { sessions, .. } = call(
            &connection,
            51,
            Method::SessionList {
                query: None,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("list")
        };
        let after = crate::durable::conversation_store_opens_on_this_thread();
        assert_eq!(after, before, "listing reads the persisted projection");
        let row = sessions
            .iter()
            .find(|row| row.id == target.session_id)
            .unwrap();
        assert_eq!(row.preview.as_deref(), Some(expected.as_str()));
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        f.gates[0].release();
        await_attempt_settled(&connection, &target.session_id).await;
        // P06 (runtime plane): a second turn is an ordinary new commit, but
        // the one-shot publisher is spent — the settled first line survives
        // byte-identically and no catalog commit happens.
        let reply = call(
            &connection,
            52,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("a follow-up repaints nothing"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        await_attempt_settled(&connection, &target.session_id).await;
        {
            let probe = probe.borrow();
            assert!(probe.finished && probe.published);
        }
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            53,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview.as_deref(), Some(expected.as_str()));
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation,
            "a later turn never commits the catalog again"
        );
        f.close().await;
    })
    .await;
}

// P07 (runtime plane): a Session named before its first turn keeps the name
// and gains the first turn's projection underneath it.
#[tokio::test]
async fn a_projection_published_underneath_a_name_survives_over_the_wire() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Session { session } = call(
            &connection,
            60,
            Method::SessionName {
                session_id: f.sessions[0].id.clone(),
                name: "kept through publication".into(),
            },
        )
        .await
        else {
            panic!("name")
        };
        assert_eq!(session.name.as_deref(), Some("kept through publication"));
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let reply = call(
            &connection,
            61,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("named session first subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            62,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.name.as_deref(), Some("kept through publication"));
        assert_eq!(
            summary.preview.as_deref(),
            Some("named session first subject")
        );
        f.gates[0].release();
        await_attempt_settled(&connection, &target.session_id).await;
        f.close().await;
    })
    .await;
}

// P08 (runtime plane): once the root projection is settled, an agent-sourced
// boundary admitted to the same root runtime is just another commit — the
// projection stays byte-identical and the catalog generation never moves.
#[tokio::test]
async fn an_agent_sourced_boundary_never_repaints_the_settled_projection() {
    bounded(async {
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let reply = call(
            &connection,
            63,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("human root subject"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        assert!(probe.borrow().published);
        f.gates[0].release();
        await_attempt_settled(&connection, &target.session_id).await;
        let generation = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .document_generation();
        let live = f.manager.load(&target.session_id, None).await.unwrap();
        let runtime = live.inspect_runtime().expect("the runtime stays resident");
        runtime
            .submit_sourced_inbound(
                crate::message::types::UserSource::Agent {
                    agent_id: crate::runtime::identity::AgentId::new("parent-agent"),
                },
                input("an agent-authored repaint attempt"),
            )
            .unwrap();
        await_attempt_settled(&connection, &target.session_id).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            64,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview.as_deref(), Some("human root subject"));
        assert_eq!(
            f.manager
                .sessions
                .catalog
                .lock()
                .await
                .document_generation(),
            generation
        );
        f.close().await;
    })
    .await;
}

// P11: a failed projection commit (one-shot catalog write fault armed before
// the first turn) leaves canonical history untouched, is observable on the
// publisher probe, lists as `None`, and the explicit repair seam publishes
// exactly the first line afterwards.
#[tokio::test]
async fn a_failed_projection_commit_preserves_history_and_repairs_exactly_once() {
    bounded(async {
        use crate::durable::ConversationStore as _;
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        // Arm before the attach so no catalog write can interleave between
        // arming and the publisher's commit.
        f.manager
            .sessions
            .catalog
            .lock()
            .await
            .arm_write_fault_before_rename();
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let reply = call(
            &connection,
            65,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("recovered after the fault"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(probe.attempted, "the commit rendered a line");
            assert!(!probe.published, "the armed fault failed the commit");
        }
        f.gates[0].release();
        await_attempt_settled(&connection, &target.session_id).await;
        // Canonical history is exactly the turn: one ordinary user boundary
        // and one assistant response.
        let path = f
            .manager
            .sessions
            .catalog
            .lock()
            .await
            .database_path(&target.session_id, &target.conversation_id);
        let store =
            crate::durable::SqliteConversationStore::open(target.conversation_id.clone(), &path)
                .unwrap();
        let canonical = store.load_canonical().unwrap();
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(
                    message,
                    crate::message::types::MessageBlock::User(user)
                        if user.kind == crate::message::types::InboundKind::Message
                ))
                .count(),
            1
        );
        assert_eq!(
            canonical
                .iter()
                .filter(|message| matches!(
                    message,
                    crate::message::types::MessageBlock::Assistant(_)
                ))
                .count(),
            1
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            66,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        // The fault was one-shot; the explicit repair seam derives and
        // publishes exactly the first line.
        assert_eq!(
            f.manager
                .sessions
                .repair_display_preview(&target.session_id)
                .await
                .unwrap(),
            crate::local_runtime::session_controller::DisplayPreviewRepair::Published
        );
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            67,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(
            summary.preview.as_deref(),
            Some("recovered after the fault")
        );
        f.close().await;
    })
    .await;
}

// Image-only case (extra, app-server plane): a first boundary with no
// renderable text — an upload-only turn — settles the projection as absent
// through the publisher itself, and a later text turn never repaints it:
// the one-shot publisher is spent, repair/report seams treat the settled
// `None` as final.
#[tokio::test]
async fn an_upload_only_first_turn_settles_the_projection_absent_and_never_repaints() {
    bounded(async {
        use base64::Engine as _;
        let f = Fixture::new().await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let mut probe = crate::local_runtime::session_display_projection::display_projection_probe(
            &target.session_id,
        )
        .expect("root composition armed the display-projection publisher");
        let MethodResult::SessionUploaded { files } = call(
            &connection,
            70,
            Method::SessionUpload {
                target: target.clone(),
                files: vec![UploadBytes {
                    name: "pixel.png".into(),
                    data: base64::engine::general_purpose::STANDARD
                        .encode([137_u8, 80, 78, 71, 13, 10, 26, 10]),
                }],
            },
        )
        .await
        else {
            panic!("upload")
        };
        let reply = call(
            &connection,
            71,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Upload(files[0].receipt.clone())],
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        f.gates[0].wait_entered().await;
        probe
            .wait_for(|probe| probe.finished)
            .await
            .expect("the publisher outlives its runtime");
        {
            let probe = probe.borrow();
            assert!(!probe.attempted, "no renderable text means no attempt");
            assert!(!probe.published);
        }
        f.gates[0].release();
        await_attempt_settled(&connection, &target.session_id).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            72,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        // The one-shot publisher is spent: a later text turn commits, but
        // the settled `None` never gets repainted.
        let reply = call(
            &connection,
            73,
            Method::TurnStart {
                target: target.clone(),
                content: wire_text("text after the upload"),
            },
        )
        .await;
        assert!(matches!(reply, MethodResult::InboundAccepted { .. }));
        await_attempt_settled(&connection, &target.session_id).await;
        let MethodResult::SessionSummary { summary } = call(
            &connection,
            74,
            Method::SessionSummary {
                session_id: target.session_id.clone(),
            },
        )
        .await
        else {
            panic!("summary")
        };
        assert_eq!(summary.preview, None);
        f.close().await;
    })
    .await;
}
