//! Direct connection contracts. Provider gates prove overlap; no socket timing.
use super::{Fixture, bounded, input};
use crate::app_server::connection::AppServerConnection;
use crate::app_server::protocol::*;
use crate::runtime_client::event::RuntimeClientEvent;

struct AskPolicy;
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
            protocol_version: 1,
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
        let connection = AppServerConnection::new(f.manager.clone());
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
                    content: input("never execute")
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
        let connection = AppServerConnection::new(f.manager.clone());
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
        let connection = AppServerConnection::new(f.manager.clone());
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
        call(&connection, 2, Method::SessionSnapshot { target: new }).await;
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
        let connection = Arc::new(AppServerConnection::new(f.manager.clone()));
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
                Method::DefaultsRead {
                    target: operation_target,
                    scope: crate::runtime_client::settings::DefaultScope::User,
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
                    content: input("must not run")
                }
            )
            .await,
            ErrorData::StaleRuntime
        );
        probe.before_operation.release();
        assert!(matches!(
            operation.await.unwrap(),
            MethodResult::Defaults { .. }
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
        let connection = AppServerConnection::new(f.manager.clone());
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
                        content: input("never execute")
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
            rejected(&connection, Method::ResourcesReload { target: old }).await,
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
        let connection = AppServerConnection::new(f.manager.clone());
        initialize(&connection).await;
        let sessions = cold_sessions(&f, 34).await;
        let mut targets = Vec::new();
        for session in &sessions[..32] {
            targets.push(attach_session(&connection, session).await);
        }
        assert_eq!(connection.attachment_counts(), (32, 0));
        assert_eq!(
            rejected(
                &connection,
                Method::SessionAttach {
                    session_id: sessions[32].id.clone(),
                    node_id: None
                }
            )
            .await,
            ErrorData::InvalidState
        );
        assert_eq!(
            f.manager
                .probe(&sessions[32].active_conversation_id)
                .compositions
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            f.manager.residency(&sessions[32].active_conversation_id),
            super::ResidencyState::Unloaded
        );
        for target in targets.drain(..2) {
            call(&connection, 301, Method::SessionUnload { target }).await;
        }
        assert_eq!(connection.attachment_counts(), (30, 0));
        // No next_notification call: unload's response is the terminal acknowledgement.
        for session in &sessions[32..] {
            targets.push(attach_session(&connection, session).await);
        }
        assert_eq!(connection.attachment_counts(), (32, 0));
        assert!(f.provider.request_bodies().is_empty());
        for target in targets {
            call(&connection, 302, Method::SessionUnload { target }).await;
        }
        assert_eq!(connection.attachment_counts(), (0, 0));
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_final_slot_is_reserved_before_composition() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.manager.clone()));
        initialize(&connection).await;
        let sessions = cold_sessions(&f, 33).await;
        for session in &sessions[..31] {
            attach_session(&connection, session).await;
        }
        let probes = [
            f.manager.probe(&sessions[31].active_conversation_id),
            f.manager.probe(&sessions[32].active_conversation_id),
        ];
        for probe in &probes {
            probe.before_compose.arm();
        }
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for session in &sessions[31..] {
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
        assert_eq!(connection.attachment_counts(), (31, 1));
        let loser = tasks.remove(1 - winner).await.unwrap();
        assert!(matches!(
            loser,
            Response::Failure(Failure {
                error: RpcError {
                    data: Some(ErrorData::InvalidState),
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
        probes[winner].before_compose.release();
        assert!(matches!(
            tasks.remove(0).await.unwrap(),
            Response::Success(_)
        ));
        assert_eq!(connection.attachment_counts(), (32, 0));
        assert!(f.provider.request_bodies().is_empty());
        for session in &sessions {
            f.manager
                .unload(&session.active_conversation_id)
                .await
                .unwrap();
        }
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_attach_releases_reservation_and_abandoned_operation_drains() {
    bounded(async {
        use std::sync::Arc;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.manager.clone()));
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
        probe.before_compose.release();
        let target = attach(&connection, &f, 0).await;
        probe.before_operation.arm();
        let worker = connection.clone();
        let operation_target = target.clone();
        let operation = tokio::spawn(async move {
            call(
                &worker,
                501,
                Method::DefaultsRead {
                    target: operation_target,
                    scope: crate::runtime_client::settings::DefaultScope::User,
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
        let connection = AppServerConnection::new(f.manager.clone());
        let before = connection.handle_json(r#"{"jsonrpc":"2.0","id":0,"method":"server/info","params":{}}"#).await.unwrap();
        assert!(matches!(before, Response::Failure(Failure { error: RpcError { data: Some(ErrorData::NotInitialized), .. }, .. })));
        let bad_version = connection.handle_json(r#"{"jsonrpc":"2.0","id":"version","method":"initialize","params":{"protocol_version":99,"client":{"name":"test","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#).await.unwrap();
        let Response::Failure(failure) = bad_version else { panic!("version mismatch") };
        assert_eq!(failure.id, Some(RequestId::String("version".into())));
        assert!(matches!(failure.error.data, Some(ErrorData::UnsupportedVersion { .. })));
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
            (r#"{"jsonrpc":"2.0","id":6,"method":"settings/saveDefault","params":{"target":{"session_id":"s","conversation_id":"c","runtime_incarnation":1,"attachment_id":"a"},"scope":"user","expected_revision":"r","setting":"settings/saveDefault"}}"#, -32602),
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
        let connection = AppServerConnection::new(f.manager.clone());
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
        settings.no_builtin_tools = true;
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
        let connection = AppServerConnection::new(f.manager.clone());
        initialize(&connection).await;
        let (a, b) = tokio::join!(attach(&connection, &f, 0), attach(&connection, &f, 1));
        let (reply_a, reply_b) = tokio::join!(
            call(
                &connection,
                20,
                Method::TurnStart {
                    target: a.clone(),
                    content: input("request-A")
                }
            ),
            call(
                &connection,
                21,
                Method::TurnStart {
                    target: b.clone(),
                    content: input("request-B")
                }
            ),
        );
        assert!(matches!(reply_a, MethodResult::InboundAccepted { .. }));
        assert!(matches!(reply_b, MethodResult::InboundAccepted { .. }));
        tokio::join!(f.gates[0].wait_entered(), f.gates[1].wait_entered());
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
        let rival = AppServerConnection::new(f.manager.clone());
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
        let MethodResult::Snapshot { snapshot, .. } =
            call(&rival, 32, Method::SessionSnapshot { target: new_a }).await
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
        let connection = AppServerConnection::new(f.manager.clone());
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
                    content: input("must not execute"),
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
                let connection = Arc::new(AppServerConnection::new(f.manager.clone()));
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
                        content: input("request-A"),
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
        let connection = Arc::new(AppServerConnection::new(f.manager.clone()));
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
