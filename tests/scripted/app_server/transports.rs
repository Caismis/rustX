//! Native execution remains authoritative across real adapter disconnects.
use super::app_server_conformance;
use super::{Fixture, bounded, input};
use crate::app_server::{
    connection::AppServerConnection,
    protocol::*,
    transport::{self, stdio, websocket},
};
use futures_util::SinkExt;
use std::{io, sync::Arc};
use tokio_util::sync::CancellationToken;
#[path = "../../support/app_server_driver.rs"]
mod driver;
use app_server_conformance::AppServerConformanceDriver;

async fn connected(
    f: &Fixture,
    ws: bool,
) -> (driver::Driver, tokio::task::JoinHandle<io::Result<()>>) {
    let manager = f.host.clone();
    if ws {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            websocket::connection(
                listener.accept().await.unwrap().0,
                manager,
                websocket::Credential::new(driver::TOKEN.into()).unwrap(),
                CancellationToken::new(),
            )
            .await
        });
        (driver::websocket(&url).await, server)
    } else {
        let (client, server) = tokio::io::duplex(65536);
        let (reader, writer) = tokio::io::split(server);
        let server = tokio::spawn(stdio::serve(
            Arc::new(AppServerConnection::new(manager)),
            reader,
            writer,
            CancellationToken::new(),
        ));
        let (reader, writer) = tokio::io::split(client);
        (driver::jsonl(reader, writer), server)
    }
}
async fn call(client: &impl AppServerConformanceDriver, id: i64, call: Method) -> MethodResult {
    let Response::Success(response) = client
        .request(Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .await
    else {
        panic!("success expected");
    };
    response.result
}
async fn initialize(client: &impl AppServerConformanceDriver) {
    call(
        client,
        0,
        Method::Initialize(InitializeParams {
            protocol_version: 22,
            client: ClientIdentity {
                name: "transport".into(),
                version: "1".into(),
            },
            presentation: PresentationCapabilities::default(),
        }),
    )
    .await;
}
async fn attach(client: &impl AppServerConformanceDriver, f: &Fixture) -> AttachmentTarget {
    let MethodResult::Attached { target, .. } = call(
        client,
        1,
        Method::SessionAttach {
            session_id: f.sessions[0].id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("attached");
    };
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapter_disconnect_preserves_active_execution_and_replacement_authority() {
    bounded(async {
        for ws in [false, true] {
            let f = Fixture::new().await;
            let (client, serving) = connected(&f, ws).await;
            initialize(&client).await;
            let old = attach(&client, &f).await;
            call(
                &client,
                2,
                Method::TurnStart {
                    target: old.clone(),
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
            client.close().await;
            let _ = serving.await.unwrap(); // deterministic attachment settlement, no socket timing assumption
            let runtime = f.manager.load(&old.session_id, None).await.unwrap();
            assert!(runtime.inspect_runtime().unwrap().has_current_attempt());
            assert_eq!(runtime.incarnation_id(), old.runtime_incarnation);
            let replacement = AppServerConnection::new(f.host.clone());
            let direct = app_server_conformance::DirectDriver(&replacement);
            initialize(&direct).await;
            let new = attach(&direct, &f).await;
            assert_ne!(old.attachment_id, new.attachment_id);
            assert_eq!(new.runtime_incarnation, old.runtime_incarnation);
            let stale = direct
                .request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(3),
                    call: Method::TurnCancel { target: old },
                })
                .await;
            assert!(matches!(stale, Response::Failure(_)));
            f.gates[0].release();
            loop {
                if let NotificationMethod::Event { event, .. } =
                    direct.next_notification().await.notification
                    && matches!(
                        *event,
                        crate::runtime_client::event::RuntimeClientEvent::AttemptSettled { .. }
                    )
                {
                    break;
                }
            }
            replacement.close();
            f.close().await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapter_disconnect_cannot_settle_approval_or_questionnaire() {
    bounded(async {
        for ws in [false, true] {
            for tool in ["read", "ask_user"] {
                let mut f = Fixture::with_tool(Some(tool)).await;
                let clock = Arc::new(crate::runtime::monotonic::ManualMonotonicClock::new());
                f.manager.clock = clock.clone();
                let (client, serving) = connected(&f, ws).await;
                initialize(&client).await;
                let target = attach(&client, &f).await;
                let runtime = f.manager.load(&target.session_id, None).await.unwrap();
                let native = runtime.inspect_runtime().unwrap();
                if tool == "read" {
                    native.install_test_pre_tool_policy(Arc::new(super::protocol::AskPolicy));
                }
                let coordinator = native.interaction_test_owner();
                call(
                    &client,
                    2,
                    Method::TurnStart {
                        target,
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
                f.gates[0].release();
                coordinator.pending_published.notified().await;
                assert_eq!(coordinator.pending_count(), 1);
                client.close().await;
                let _ = serving.await.unwrap();
                assert_eq!(coordinator.pending_count(), 1);
                assert!(native.idle_epoch().is_err());
                clock.advance(f.manager.policy().idle_grace_ms + 1);
                f.manager.reap_idle();
                assert_eq!(
                    f.manager.residency(runtime.conversation_id()),
                    super::ResidencyState::Loaded
                );
                let replacement = AppServerConnection::new(f.host.clone());
                let direct = app_server_conformance::DirectDriver(&replacement);
                initialize(&direct).await;
                let target = attach(&direct, &f).await;
                let MethodResult::Snapshot { snapshot, .. } = call(
                    &direct,
                    3,
                    Method::SessionSnapshot {
                        trace_records: vec![],
                        target,
                    },
                )
                .await
                else {
                    panic!("snapshot");
                };
                assert_eq!(snapshot.pending_interactions.len(), 1);
                replacement.close();
                f.close().await;
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_pipe_overflows_and_drops_writer_while_runtime_progresses() {
    bounded(async {
        use tokio::io::AsyncWriteExt;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        let direct = app_server_conformance::DirectDriver(&connection);
        initialize(&direct).await;
        let target = attach(&direct, &f).await;
        call(
            &direct,
            2,
            Method::TurnStart {
                target: target.clone(),
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
        let (mut input, reader) = tokio::io::duplex(65536);
        let (writer, _unread) = tokio::io::duplex(1);
        let server = tokio::spawn(stdio::serve(
            connection.clone(),
            reader,
            writer,
            CancellationToken::new(),
        ));
        input
            .write_all("{\n".repeat(transport::OUTBOUND_MESSAGES + 2).as_bytes())
            .await
            .unwrap();
        let error = server.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("outbound capacity"));
        assert_eq!(connection.attachment_counts(), (0, 0));
        let native = f.manager.load(&target.session_id, None).await.unwrap();
        assert!(native.inspect_runtime().unwrap().has_current_attempt());
        f.gates[0].release();
        f.close().await;
    })
    .await;
}

#[tokio::test]
async fn blocked_websocket_overflows_with_controlled_duplex_capacity() {
    bounded(async {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let f = Fixture::new().await;
        // Real WebSocket HTTP handshake and framing over an exactly bounded byte pipe.
        let (client, server) = tokio::io::duplex(1024);
        let manager = f.host.clone();
        let serving = tokio::spawn(websocket::connection(
            server,
            manager,
            websocket::Credential::new(driver::TOKEN.into()).unwrap(),
            CancellationToken::new(),
        ));
        let mut request = "ws://localhost/".into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            format!("rustx.app-server.v22, rustx-token.{}", driver::TOKEN)
                .parse()
                .unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::client_async(request, client)
            .await
            .unwrap();
        // Each response is > pipe capacity. Never poll the client reader.
        let record =
            serde_json::json!({"jsonrpc":"2.0", "id":"x".repeat(2048), "method":"unknown"})
                .to_string();
        let feed = async {
            for _ in 0..transport::OUTBOUND_MESSAGES + 2 {
                if socket.send(record.clone().into()).await.is_err() {
                    break;
                }
            }
        };
        let (result, ()) = tokio::join!(serving, feed);
        assert!(
            result
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("outbound capacity")
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapters_share_direct_semantic_expectations() {
    bounded(async {
        for ws in [false, true] {
            let f = Fixture::new().await;
            let (client, server) = connected(&f, ws).await;
            app_server_conformance::representative_scenario(
                &client,
                [f.sessions[0].id.clone(), f.sessions[1].id.clone()],
            )
            .await;
            client.close().await;
            let _ = server.await.unwrap();
            f.close().await;
        }
    })
    .await;
}

#[tokio::test]
async fn blocked_writer_deadline_is_cancellable_without_another_input_record() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::new().await;
    let connection = Arc::new(AppServerConnection::new(f.host.clone()));
    let (mut input, reader) = tokio::io::duplex(1024);
    let (writer, mut unread) = tokio::io::duplex(1);
    tokio::time::pause();
    let serving = tokio::spawn(stdio::serve(
        connection.clone(),
        reader,
        writer,
        CancellationToken::new(),
    ));
    input.write_all(b"{\n").await.unwrap();
    unread.read_u8().await.unwrap(); // writer has entered the deadline-protected I/O
    tokio::time::advance(transport::WRITE_TIMEOUT).await;
    assert!(
        serving
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("deadline")
    );
    assert_eq!(connection.attachment_counts(), (0, 0));
    tokio::time::resume();
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_admission_overflow_releases_transport_but_not_server_operation_ownership() {
    bounded(async {
        use tokio::io::AsyncWriteExt;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        let direct = app_server_conformance::DirectDriver(&connection);
        initialize(&direct).await;
        let target = attach(&direct, &f).await;
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let (mut input, reader) = tokio::io::duplex(65536);
        let (writer, _output) = tokio::io::duplex(65536);
        let serving = tokio::spawn(stdio::serve(
            connection.clone(),
            reader,
            writer,
            CancellationToken::new(),
        ));
        for id in 0..=transport::IN_FLIGHT_REQUESTS {
            let request = Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(i64::try_from(id).unwrap()),
                call: Method::SessionSnapshot {
                    trace_records: vec![],
                    target: target.clone(),
                },
            };
            input
                .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
                .await
                .unwrap();
        }
        probe.before_operation.entered().await;
        assert!(
            serving
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("request capacity")
        );
        assert_eq!(connection.attachment_counts(), (0, 0));
        probe.before_operation.release();
        f.close().await;
    })
    .await;
}

struct BackgroundGate {
    gate: Arc<super::AsyncGate>,
    cancellation:
        Arc<std::sync::Mutex<Option<crate::runtime::cancellation::ExecutionCancellation>>>,
}
impl crate::tools::executor::ToolExecutor for BackgroundGate {
    fn start<'a>(
        &'a self,
        _: crate::tools::types::ToolInvocation,
        context: crate::tools::executor::ToolExecutionContext<'a>,
    ) -> crate::tools::executor::ToolExecutionHandle<'a> {
        *self.cancellation.lock().unwrap() = Some(context.cancellation.clone());
        let gate = self.gate.clone();
        crate::tools::executor::ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                gate.park().await;
                crate::tools::types::ToolExecutionResult {
                    status: crate::tools::types::ToolExecutionStatus::Success,
                    content: Vec::new(),
                    duration_ms: 0,
                    exit_code: None,
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                }
            }),
            context.cancellation.clone(),
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapter_disconnect_does_not_cancel_server_owned_background_execution() {
    bounded(async {
        for ws in [false, true] {
            let mut f = Fixture::new().await;
            let clock = Arc::new(crate::runtime::monotonic::ManualMonotonicClock::new());
            f.manager.clock = clock.clone();
            let (client, serving) = connected(&f, ws).await;
            initialize(&client).await;
            let target = attach(&client, &f).await;
            let managed = f.manager.load(&target.session_id, None).await.unwrap();
            let native = managed.inspect_runtime().unwrap();
            let gate = Arc::new(super::AsyncGate::default());
            gate.arm();
            let cancellation = Arc::new(std::sync::Mutex::new(None));
            let executor: Arc<dyn crate::tools::executor::ToolExecutor> =
                Arc::new(BackgroundGate {
                    gate: gate.clone(),
                    cancellation: cancellation.clone(),
                });
            let invocation = crate::tools::types::ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: crate::runtime::identity::ToolCallId::new("background-transport"),
                },
                tool_id: crate::runtime::identity::ToolId::new("background-transport"),
                tool_name: "background-transport".into(),
                mode: crate::tools::types::ToolInvocationMode::Background,
                arguments: serde_json::json!({}),
            };
            let background = native.tool_runtime().background();
            let prepared = background
                .prepare_dispatch(
                    &invocation,
                    &executor,
                    crate::tools::environment::ToolEnvironment::new(),
                )
                .unwrap();
            let crate::tools::background::BackgroundDispatchOutcome::Accepted {
                execution_id, ..
            } = background
                .commit_dispatch(prepared, &crate::runtime::CancellationSignal::new())
                .unwrap()
            else {
                panic!("accepted");
            };
            gate.entered().await;
            client.close().await;
            let _ = serving.await.unwrap();
            assert!(
                !cancellation
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_cancelled()
            );
            assert!(native.idle_epoch().is_err());
            clock.advance(f.manager.policy().idle_grace_ms + 1);
            f.manager.reap_idle();
            assert_eq!(
                f.manager.residency(managed.conversation_id()),
                super::ResidencyState::Loaded
            );
            gate.release();
            background.wait_until_terminal(&execution_id).await.unwrap();
            assert!(
                !cancellation
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_cancelled()
            );
            f.close().await;
        }
    })
    .await;
}

#[tokio::test]
async fn incomplete_websocket_handshake_has_a_finite_deadline() {
    let f = Fixture::new().await;
    let (_silent_client, server) = tokio::io::duplex(1024);
    tokio::time::pause();
    let result = websocket::connection(
        server,
        f.host.clone(),
        websocket::Credential::new(driver::TOKEN.into()).unwrap(),
        CancellationToken::new(),
    )
    .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("handshake deadline")
    );
    tokio::time::resume();
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_pending_session_request_does_not_serialize_another_session() {
    bounded(async {
        for ws in [false, true] {
            let f = Fixture::new().await;
            let (client, server) = connected(&f, ws).await;
            initialize(&client).await;
            let a = attach(&client, &f).await;
            let MethodResult::Attached { target: b, .. } = call(
                &client,
                2,
                Method::SessionAttach {
                    session_id: f.sessions[1].id.clone(),
                    node_id: None,
                },
            )
            .await
            else {
                panic!("attached B");
            };
            let probe = f.manager.probe(&a.conversation_id);
            probe.before_operation.arm();
            let pending = call(
                &client,
                3,
                Method::SessionSnapshot {
                    trace_records: vec![],
                    target: a,
                },
            );
            let independent = async {
                probe.before_operation.entered().await;
                assert!(matches!(
                    call(
                        &client,
                        4,
                        Method::SessionSnapshot {
                            trace_records: vec![],
                            target: b
                        }
                    )
                    .await,
                    MethodResult::Snapshot { .. }
                ));
                probe.before_operation.release();
            };
            let (result, ()) = tokio::join!(pending, independent);
            assert!(matches!(result, MethodResult::Snapshot { .. }));
            client.close().await;
            let _ = server.await.unwrap();
            f.close().await;
        }
    })
    .await;
}

#[tokio::test]
async fn authenticated_websocket_capacity_is_released_after_client_reaping() {
    bounded(async {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let f = Fixture::new().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let (slots, mut observed) = tokio::sync::watch::channel(0);
        let serving = tokio::spawn(websocket::serve_listener(
            listener,
            f.host.clone(),
            websocket::Credential::new(driver::TOKEN.into()).unwrap(),
            stop.clone(),
            Some(slots),
        ));
        let mut clients = Vec::new();
        for _ in 0..websocket::MAX_CLIENTS {
            clients.push(driver::socket(&url).await);
        }
        observed
            .wait_for(|count| *count == websocket::MAX_CLIENTS)
            .await
            .unwrap();
        let mut request = url.as_str().into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            format!("rustx.app-server.v22, rustx-token.{}", driver::TOKEN)
                .parse()
                .unwrap(),
        );
        let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
        // The listener drops excess sockets before HTTP admission, rather than
        // returning the HTTP 401 used for invalid credentials.
        assert!(matches!(
            error,
            tokio_tungstenite::tungstenite::Error::Io(_)
                | tokio_tungstenite::tungstenite::Error::Protocol(
                    tokio_tungstenite::tungstenite::error::ProtocolError::HandshakeIncomplete
                )
        ));
        let mut released = clients.pop().unwrap();
        released.close(None).await.unwrap();
        observed
            .wait_for(|count| *count == websocket::MAX_CLIENTS - 1)
            .await
            .unwrap();
        clients.push(driver::socket(&url).await);
        observed
            .wait_for(|count| *count == websocket::MAX_CLIENTS)
            .await
            .unwrap();
        stop.cancel();
        serving.await.unwrap().unwrap();
        f.close().await;
    })
    .await;
}

// A synchronous writer keeps draining and replenishes one native notification
// per observed notification. Thus queue overflow cannot mask scheduling starvation.
struct ReplenishingWriter {
    native: crate::runtime::ConversationRuntime,
    record: Vec<u8>,
    notifications: usize,
    stop: CancellationToken,
}
impl tokio::io::AsyncWrite for ReplenishingWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        self.record.extend_from_slice(bytes);
        if self.record.last() == Some(&b'\n') {
            let value: serde_json::Value = serde_json::from_slice(&self.record).unwrap();
            self.record.clear();
            if value.get("id").is_some() {
                assert_eq!(value["id"], 99);
                assert!(value.get("result").is_some(), "{value}");
                assert!(self.notifications > 0, "observations also make progress");
                self.stop.cancel();
            } else {
                self.notifications += 1;
                assert!(
                    self.notifications <= transport::IN_FLIGHT_REQUESTS + 1,
                    "ready control request was starved by observations"
                );
                let mut selection = self.native.model_view().configured;
                selection.request_params.insert(
                    "temperature".into(),
                    serde_json::json!(
                        f64::from(u32::try_from(self.notifications).unwrap()) / 100.0
                    ),
                );
                self.native.model_set(selection).unwrap();
            }
        }
        std::task::Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn ready_control_admission_does_not_wait_for_continuous_notifications() {
    bounded(async {
        use tokio::io::AsyncWriteExt;
        let f = Fixture::new().await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        let direct = app_server_conformance::DirectDriver(&connection);
        initialize(&direct).await;
        let target = attach(&direct, &f).await;
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let native = managed.inspect_runtime().unwrap();
        // The ready backlog exceeds the fairness bound and is replenished by the
        // writer. Input is already in the pipe before the shared serve core is polled.
        for i in 0..64 {
            let mut selection = native.model_view().configured;
            selection.request_params.insert(
                "temperature".into(),
                serde_json::json!(f64::from(i) / 100.0),
            );
            native.model_set(selection).unwrap();
        }
        let (mut client, input) = tokio::io::duplex(4096);
        let request = Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(99),
            call: Method::SessionDetach { target },
        };
        client
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        let stop = CancellationToken::new();
        stdio::serve(
            connection.clone(),
            input,
            ReplenishingWriter {
                native,
                record: Vec::new(),
                notifications: 0,
                stop: stop.clone(),
            },
            stop,
        )
        .await
        .unwrap();
        assert_eq!(connection.attachment_counts(), (0, 0));
        f.close().await;
    })
    .await;
}
