//! Request identity, initialization and admission at the direct connection:
//! strict wire decoding, negotiated identity, reserved request ids and host
//! request ownership that outlives a dropped protocol waiter.
#![allow(clippy::large_futures)] // bounded fixture futures; no recursive or unbounded stack growth
use super::*;

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
        assert!(matches!(failure.error.data, Some(ErrorData::UnsupportedVersion { supported: APP_SERVER_PROTOCOL_VERSION, requested: 12 })));
        for requested in 0..crate::app_server::protocol::APP_SERVER_PROTOCOL_VERSION {
            let request = serde_json::json!({"jsonrpc":"2.0","id":"previous-generation",
                "method":"initialize","params":{"protocol_version":requested,
                "client":{"name":"test","version":"1"},
                "presentation":{"images":false,"questionnaires":false,"reviews":false}}});
            let response = connection.handle_json(&request.to_string()).await.unwrap();
            assert!(matches!(response, Response::Failure(Failure { error: RpcError {
                data: Some(ErrorData::UnsupportedVersion { supported: APP_SERVER_PROTOCOL_VERSION, requested: rejected }), .. }, .. }) if rejected == requested));
            assert_eq!(rejected(&connection, Method::SessionAttach { session_id: f.sessions[0].id.clone(), node_id: None }).await, ErrorData::NotInitialized);
            assert_eq!(rejected(&connection, Method::ServerInfo {}).await, ErrorData::NotInitialized);
            assert_eq!(connection.attachment_counts(), (0, 0));
        }
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

#[tokio::test]
async fn host_request_owner_outlives_dropped_protocol_waiter() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
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
                Method::SessionCheckpoint {
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

#[tokio::test]
async fn ux04_initialize_identity_is_owned_by_host_not_connection_or_endpoint() {
    let f = Fixture::with_session_count(None, 0).await;
    let replacement = crate::app_server::host::AppServerHost::new(
        f.manager.clone(),
        crate::local_runtime::app_server_policy::AppServerPolicy::default(),
    );
    let mut identities = Vec::new();
    for host in [f.host.clone(), f.host.clone(), replacement] {
        let connection = AppServerConnection::new(host);
        let result = call(
            &connection,
            0,
            Method::Initialize(InitializeParams {
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                client: ClientIdentity {
                    name: "identity-test".into(),
                    version: "1".into(),
                },
                presentation: PresentationCapabilities::default(),
            }),
        )
        .await;
        let MethodResult::Initialized { authority_id, .. } = result else {
            panic!("initialize result")
        };
        assert!(!authority_id.is_empty());
        identities.push(authority_id);
        connection.close();
    }
    assert_eq!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
    assert!(f.provider.request_bodies().is_empty());
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_compaction_correlation_is_bounded_before_admission() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let target = attach(&connection, &f, 0).await;
        let schema = crate::app_server::schema::protocol_schema();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let identity = &schema["$defs"]["ManualCompactionRequestId"];
        assert_eq!(identity["maxLength"], 64);
        assert_eq!(identity["minLength"], 1);
        assert_eq!(identity["pattern"], r"^[A-Za-z0-9_-]+$(?![\s\S])");
        let read = || {
            call(
                &connection,
                20,
                Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![],
                },
            )
        };
        for (value, valid) in [
            ("550e8400-e29b-41d4-a716-446655440000".to_owned(), true),
            ("a".repeat(64), true),
            ("a".repeat(65), false),
            ("a".repeat(900_000), false),
            (String::new(), false),
            ("has space".to_owned(), false),
            ("非ASCII".to_owned(), false),
            ("trailing\n".to_owned(), false),
        ] {
            f.manager
                .configuration_host(&target.session_id)
                .unwrap()
                .projection_settled()
                .await;
            let native = f.manager.configuration_host(&target.session_id).unwrap();
            if !valid {
                native.inner.park_projection_worker();
            }
            let before = read().await;
            let mut wire = serde_json::to_value(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(30),
                call: Method::CompactContext {
                    target: target.clone(),
                    request_id: "valid".to_owned().try_into().unwrap(),
                },
            })
            .unwrap();
            wire["params"]["request_id"] = serde_json::Value::String(value.clone());
            assert_eq!(validator.is_valid(&wire), valid);
            let response = connection.handle_json(&wire.to_string()).await.unwrap();
            let Response::Failure(failure) = response else {
                panic!("empty context cannot compact")
            };
            if valid {
                assert_ne!(failure.error.code, -32602);
                let MethodResult::Checkpoint {
                    checkpoint: checkpoint_18,
                } = read().await
                else {
                    panic!("snapshot")
                };
                let snapshot = Box::new(checkpoint_18.snapshot);
                let result = snapshot.context.manual_compaction.unwrap();
                assert_eq!(result.request_id.unwrap().as_str(), value);
                assert!(result.released);
                assert!(result.error.is_some());
            } else {
                assert_eq!(failure.error.code, -32602);
                // Equal cursor and full context prove no start or correlation mutation.
                assert_eq!(
                    serde_json::to_value(read().await).unwrap(),
                    serde_json::to_value(before).unwrap()
                );
            }
            if !valid {
                native.inner.unpark_projection_worker();
            }
            assert!(f.provider.request_bodies().is_empty());
        }
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn t01_invalid_identity_precedes_method_and_connection_admission() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let connection = AppServerConnection::new(f.host.clone());
        for id in [
            serde_json::json!("x".repeat(255)),
            serde_json::json!(9_007_199_254_740_992_u64),
            serde_json::json!(null),
            serde_json::json!([]),
        ] {
            let record =
                serde_json::json!({"jsonrpc":"2.0", "id":id, "method":"not/a/method", "params":{}})
                    .to_string();
            let Response::Failure(failure) = connection.handle_json(&record).await.unwrap() else {
                panic!("invalid envelope")
            };
            assert_eq!(failure.id, None);
            assert_eq!(failure.error.code, -32600);
            assert_eq!(connection.attachment_counts(), (0, 0));
        }
        let Response::Failure(failure) = connection
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::String("\n".repeat(128)),
                call: Method::Initialize(InitializeParams {
                    protocol_version: APP_SERVER_PROTOCOL_VERSION,
                    client: ClientIdentity {
                        name: "rejected".into(),
                        version: "1".into(),
                    },
                    presentation: PresentationCapabilities::default(),
                }),
            })
            .await
        else {
            panic!("typed invalid identity")
        };
        assert_eq!((failure.id, failure.error.code), (None, -32600));
        // Rejected typed Initialize did not alter connection state.
        initialize(&connection).await;
        for call in [
            Method::DeliveryCancel {
                request_id: RequestId::String("x".repeat(255)),
            },
            Method::AgentObserveCancel {
                request_id: RequestId::Integer(i64::MAX),
            },
        ] {
            let Response::Failure(failure) = connection
                .handle_request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(19),
                    call,
                })
                .await
            else {
                panic!("invalid cancellation parameter")
            };
            assert_eq!(
                (failure.id, failure.error.code),
                (Some(RequestId::Integer(19)), -32602)
            );
        }
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn t20_request_identity_remains_owned_through_response_disposition() {
    bounded(async {
        let f = Fixture::with_session_count(None, 0).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let request = || Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(31),
            call: Method::ServerInfo {},
        };
        let reply = connection.reply(request()).await;
        let Response::Failure(failure) = connection.handle_request(request()).await else {
            panic!("overlapping identity must be refused")
        };
        assert_eq!(failure.error.data, Some(ErrorData::DuplicateRequestId));
        drop(reply); // Unaccepted response abandoned; exact association is released.
        assert!(matches!(
            connection.handle_request(request()).await,
            Response::Success(_)
        ));
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn t20_observation_cancel_after_admission_before_first_poll_never_dispatches() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Attached { target, .. } = call(
            &connection,
            1,
            Method::SessionAttach {
                session_id: f.sessions[0].id.clone(),
                node_id: None,
            },
        )
        .await
        else {
            panic!("attached")
        };
        let wait = connection.admitted_request_for_test(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(41),
            call: Method::AgentObserve {
                target,
                agent_id: crate::runtime::identity::AgentId::new("never-inspected"),
                after: None,
            },
        });
        // No future has been polled. Cancellation must already find the owner.
        let cancelled = call(
            &connection,
            42,
            Method::AgentObserveCancel {
                request_id: RequestId::Integer(41),
            },
        )
        .await;
        assert_eq!(
            cancelled,
            MethodResult::AgentObserveCancelled { accepted: true }
        );
        let Response::Failure(failure) = wait.await.commit() else {
            panic!("cancelled observation")
        };
        assert_eq!(failure.error.data, Some(ErrorData::ObservationCancelled));
        // Dispatching this nonexistent Child would have produced UnknownAgent.
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t20_aborted_response_waiter_retains_native_operation_reservation_until_termination() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        initialize(&connection).await;
        let MethodResult::Attached { target, .. } = call(
            &connection,
            1,
            Method::SessionAttach {
                session_id: f.sessions[0].id.clone(),
                node_id: None,
            },
        )
        .await
        else {
            panic!("attached")
        };
        let runtime = f.manager.load(&target.session_id, None).await.unwrap();
        let resident = runtime.client().current().unwrap();
        let mut operations = resident.operations.subscribe();
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let request = Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(51),
            call: Method::ModelGet { target },
        };
        let waiting = tokio::spawn(connection.admitted_request_for_test(request.clone()));
        probe.before_operation.entered().await;
        waiting.abort();
        assert!(matches!(waiting.await, Err(error) if error.is_cancelled()));
        let Response::Failure(failure) = connection.handle_request(request.clone()).await else {
            panic!("physical work still owns identity")
        };
        assert_eq!(failure.error.data, Some(ErrorData::DuplicateRequestId));
        // Native OperationLease release is the physical termination proof.
        probe.before_operation.release();
        operations.wait_for(|count| *count == 0).await.unwrap();
        assert!(matches!(
            connection.handle_request(request).await,
            Response::Success(_)
        ));
        connection.close();
        f.close().await;
    })
    .await;
}
