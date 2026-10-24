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
#[path = "driver_lifecycle.rs"]
mod driver_lifecycle;
#[path = "oversized_notification.rs"]
mod oversized_notification;
use app_server_conformance::AppServerConformanceDriver;
#[path = "bounded_delivery.rs"]
mod bounded_delivery;
#[path = "outbound_backpressure.rs"]
mod outbound_backpressure;

pub(super) async fn connected(
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
pub(super) async fn call(
    client: &impl AppServerConformanceDriver,
    id: i64,
    call: Method,
) -> MethodResult {
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
pub(super) async fn initialize(client: &impl AppServerConformanceDriver) {
    call(
        client,
        0,
        Method::Initialize(InitializeParams {
            protocol_version: APP_SERVER_PROTOCOL_VERSION,
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
    let MethodResult::Checkpoint { checkpoint } = call(
        client,
        10_000_001,
        Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: vec![],
        },
    )
    .await
    else {
        panic!("checkpoint")
    };
    call(
        client,
        20_000_001,
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: checkpoint.cursor,
        },
    )
    .await;
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t08_t12_content_range_error_is_typed_correlated_and_keeps_connection_following() {
    use crate::observation::content::{CanonicalContentComponent, ContentSource};
    use base64::Engine as _;
    bounded(async {
        // TCP WebSocket and the production Stdio framing use the same read contract.
        for ws in [false, true] {
            let fixture = Fixture::new().await;
            let (client, serving) = connected(&fixture, ws).await;
            initialize(&client).await;
            let target = attach(&client, &fixture).await;
            let text = "水🙂 exact bytes ".repeat(4096);
            let MethodResult::InboundAccepted { message_id, .. } = call(
                &client,
                46_780,
                Method::TurnStart {
                    target: target.clone(),
                    content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                        text: text.clone(),
                    })],
                },
            )
            .await
            else {
                panic!("original inbound identity");
            };
            fixture.gates[0].wait_entered().await;
            let MethodResult::Checkpoint { checkpoint } = call(
                &client,
                46_781,
                Method::SessionCheckpoint {
                    target: target.clone(),
                    trace_records: vec![],
                },
            )
            .await
            else {
                panic!("published request-start checkpoint");
            };
            let reference = checkpoint
                .snapshot
                .message_descriptors
                .iter()
                .find(|descriptor| descriptor.identity.as_ref() == Some(&message_id))
                .expect("exact original canonical message")
                .content
                .iter()
                .find(|binding| {
                    matches!(
                        binding.content_ref.source,
                        ContentSource::CanonicalMessageBlock {
                            component: CanonicalContentComponent::Payload,
                            ..
                        }
                    )
                })
                .expect("original text source")
                .content_ref
                .clone();
            assert_eq!(reference.readable_end, text.len() as u64);
            let read = |offset| Method::ConversationContentRead {
                target: target.clone(),
                agent_id: None,
                content_ref: reference.clone(),
                offset,
                max_bytes: 1,
            };
            let Response::Failure(failure) = client
                .request(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(46_782),
                    call: read(reference.readable_end.checked_add(1).unwrap()),
                })
                .await
            else {
                panic!("out-of-frontier read refused");
            };
            assert_eq!(failure.id, Some(RequestId::Integer(46_782)));
            assert_eq!(failure.error.data, Some(ErrorData::InvalidRange));
            let MethodResult::ContentChunk { chunk } = call(&client, 46_783, read(0)).await else {
                panic!("same connection remains readable");
            };
            assert_eq!(chunk.content_ref, reference);
            assert_eq!(chunk.next_offset, Some(1));
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(chunk.data_base64)
                    .unwrap(),
                &text.as_bytes()[..1],
                "a chunk may split UTF-8 without changing bytes"
            );
            let MethodResult::ContentChunk { chunk } =
                call(&client, 46_784, read(reference.readable_end)).await
            else {
                panic!("authorized range EOF");
            };
            assert!(chunk.data_base64.is_empty());
            assert!(chunk.complete);
            assert_eq!(chunk.next_offset, None);
            assert_eq!(fixture.provider.request_bodies().len(), 1);
            client.close().await;
            if let Err(error) = serving.await.unwrap() {
                // Driver::close drops the socket without a WebSocket Close frame.
                assert!(ws, "Stdio EOF must complete cleanly: {error}");
                assert!(matches!(
                    error
                        .get_ref()
                        .and_then(|source| source.downcast_ref::<
                            tokio_tungstenite::tungstenite::Error,
                        >()),
                    Some(tokio_tungstenite::tungstenite::Error::Protocol(
                        tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
                    ))
                ), "unexpected WebSocket teardown failure: {error}");
            }
            assert!(
                fixture
                    .manager
                    .load(&target.session_id, None)
                    .await
                    .unwrap()
                    .inspect_runtime()
                    .unwrap()
                    .has_current_attempt(),
                "read errors and carrier close cannot settle the held execution"
            );
            fixture.close().await;
        }
    })
    .await;
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
                    && event.facts().iter().any(|fact| {
                        matches!(
                            fact,
                            crate::runtime_client::event::RuntimeClientEvent::AttemptSettled { .. }
                        )
                    })
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
                let MethodResult::Checkpoint {
                    checkpoint: checkpoint_1,
                } = call(
                    &direct,
                    3,
                    Method::SessionCheckpoint {
                        trace_records: vec![],
                        target,
                    },
                )
                .await
                else {
                    panic!("snapshot");
                };
                let snapshot = Box::new(checkpoint_1.snapshot);
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
        let f = Fixture::with_session_count(None, 1).await;
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
        // Queue retirement cannot retract the first accepted byte. With no
        // reader, the retained remainder ends at the physical write deadline.
        assert_eq!(error.to_string(), "JSONL write deadline exceeded");
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
        let f = Fixture::with_session_count(None, 0).await;
        // Real WebSocket HTTP handshake and framing over an exactly bounded byte pipe.
        let (client, server) = tokio::io::duplex(128);
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
            format!("rustx.app-server.v57, rustx-token.{}", driver::TOKEN)
                .parse()
                .unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::client_async(request, client)
            .await
            .unwrap();
        // Each response is > pipe capacity. Never poll the client reader.
        let record = serde_json::json!({"jsonrpc":"2.0", "id":"x".repeat(254), "method":"unknown"})
            .to_string();
        let direct = AppServerConnection::new(f.host.clone());
        let response = direct.handle_json(&record).await.unwrap();
        let encoded = serde_json::to_vec(&response).unwrap();
        assert!(encoded.len() > 128 && encoded.len() <= transport::MAX_MESSAGE_BYTES);
        direct.close();
        let feed = async {
            for _ in 0..transport::OUTBOUND_MESSAGES + 2 {
                if socket.send(record.clone().into()).await.is_err() {
                    break;
                }
            }
        };
        let (result, ()) = tokio::join!(serving, feed);
        assert_eq!(
            result.unwrap().unwrap_err().to_string(),
            "WebSocket write deadline exceeded"
        );
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapters_share_direct_semantic_expectations() {
    Box::pin(bounded(async {
        for ws in [false, true] {
            let f = Fixture::new().await;
            let (client, server) = connected(&f, ws).await;
            Box::pin(app_server_conformance::representative_scenario(
                &client,
                [f.sessions[0].id.clone(), f.sessions[1].id.clone()],
            ))
            .await;
            client.close().await;
            let _ = server.await.unwrap();
            f.close().await;
        }
    }))
    .await;
}

#[tokio::test]
async fn blocked_writer_deadline_is_cancellable_without_another_input_record() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::with_session_count(None, 0).await;
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

async fn next_pipe_response(
    lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>,
) -> Response {
    loop {
        let line = lines
            .next_line()
            .await
            .unwrap()
            .expect("live pipe response");
        assert!(line.len() <= transport::MAX_MESSAGE_BYTES);
        if let Ok(response) = serde_json::from_str::<Response>(&line) {
            return response;
        }
        serde_json::from_str::<Notification>(&line).expect("native bounded notification");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn request_admission_overflow_is_correlated_and_disconnect_retains_server_operation_ownership()
 {
    bounded(async {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let f = Fixture::with_session_count(None, 1).await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        let direct = app_server_conformance::DirectDriver(&connection);
        initialize(&direct).await;
        let target = attach(&direct, &f).await;
        let probe = f.manager.probe(&target.conversation_id);
        probe.before_operation.arm();
        let (mut input, reader) = tokio::io::duplex(65536);
        let (writer, output) = tokio::io::duplex(65536);
        let serving = tokio::spawn(stdio::serve(
            connection.clone(),
            reader,
            writer,
            CancellationToken::new(),
        ));
        assert_eq!(transport::IN_FLIGHT_REQUESTS, 16);
        for id in 0..=transport::IN_FLIGHT_REQUESTS {
            let request = Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(i64::try_from(id).unwrap()),
                call: Method::SessionCheckpoint {
                    trace_records: vec![],
                    target: target.clone(),
                },
            };
            input
                .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
                .await
                .unwrap();
        }
        let mut lines = tokio::io::BufReader::new(output).lines();
        let mut refused = std::collections::BTreeSet::new();
        for _ in 6..=transport::IN_FLIGHT_REQUESTS {
            let Response::Failure(failure) = next_pipe_response(&mut lines).await else {
                panic!("correlated capacity refusal");
            };
            assert_eq!(failure.error.data, Some(ErrorData::RequestCapacity));
            let Some(RequestId::Integer(id)) = failure.id else {
                panic!("exact request id");
            };
            assert!(refused.insert(id));
        }
        assert_eq!(refused, (6..=16).collect());
        probe.before_operation.parked(6).await;
        assert_eq!(connection.attachment_counts(), (1, 0));
        assert!(
            !serving.is_finished(),
            "capacity refusal keeps the carrier alive"
        );
        let detach = Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(9000),
            call: Method::SessionDetach {
                target: target.clone(),
            },
        };
        input
            .write_all(format!("{}\n", serde_json::to_string(&detach).unwrap()).as_bytes())
            .await
            .unwrap();
        let Response::Success(detached) = next_pipe_response(&mut lines).await else {
            panic!("reserved control lane");
        };
        assert_eq!(detached.id, RequestId::Integer(9000));
        assert!(matches!(detached.result, MethodResult::Detached { .. }));
        assert_eq!(connection.attachment_counts(), (0, 0));
        assert_eq!(probe.before_operation.parked_now(), 6);
        assert!(!serving.is_finished());
        drop(input); // Actual input EOF retires transport waiters, never native operations.
        serving.await.unwrap().unwrap();
        assert_eq!(probe.before_operation.parked_now(), 6);
        assert_eq!(
            f.manager
                .diagnostics()
                .sessions
                .iter()
                .find(|row| row.conversation_id == target.conversation_id)
                .unwrap()
                .operations,
            6
        );
        probe.before_operation.release();
        probe.before_operation.parked(0).await;
        f.close().await;
    })
    .await;
}

pub(super) struct BackgroundGate {
    pub(super) gate: Arc<super::AsyncGate>,
    pub(super) cancellation:
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
                    deliveries: Vec::new(),
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
    let f = Fixture::with_session_count(None, 0).await;
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
                Method::SessionCheckpoint {
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
                        Method::SessionCheckpoint {
                            trace_records: vec![],
                            target: b
                        }
                    )
                    .await,
                    MethodResult::Checkpoint { .. }
                ));
                probe.before_operation.release();
            };
            let (result, ()) = tokio::join!(pending, independent);
            assert!(matches!(result, MethodResult::Checkpoint { .. }));
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
        let f = Fixture::with_session_count(None, 0).await;
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
            format!("rustx.app-server.v57, rustx-token.{}", driver::TOKEN)
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

// Populate the existing test-only projection input without mutating Session
// authority. This test concerns transport fairness, not model selection.
fn replenish_observation(native: &crate::runtime::ConversationRuntime) {
    native.installed_observation_bridge().unwrap().push(
        crate::runtime::observation::ConversationObservation::SessionModelChanged {
            model: native.model_read_source(),
            capability: None,
        },
    );
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
                replenish_observation(&self.native);
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
        let f = Fixture::with_session_count(None, 1).await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        let direct = app_server_conformance::DirectDriver(&connection);
        initialize(&direct).await;
        let target = attach(&direct, &f).await;
        let managed = f.manager.load(&target.session_id, None).await.unwrap();
        let native = managed.inspect_runtime().unwrap();
        // The ready backlog exceeds the fairness bound and is replenished by the
        // writer. Input is already in the pipe before the shared serve core is polled.
        for _ in 0..64 {
            replenish_observation(&native);
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

// Delivery access is a separate owner-only credential, additive to the
// ordinary transport token. It never substitutes for it, a wrong value fails
// the handshake closed, and native credential removal revokes live grants.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_delivery_access_is_a_separate_additive_revocable_credential() {
    async fn delivery_access(client: &driver::Driver) -> bool {
        let MethodResult::Initialized { capabilities, .. } = call(
            client,
            0,
            Method::Initialize(InitializeParams {
                protocol_version: APP_SERVER_PROTOCOL_VERSION,
                client: ClientIdentity {
                    name: "rustx-product-host".into(),
                    version: "1".into(),
                },
                presentation: PresentationCapabilities::default(),
            }),
        )
        .await
        else {
            panic!("initialized")
        };
        capabilities.delivery_access
    }
    async fn read(client: &driver::Driver, target: AttachmentTarget) -> ErrorData {
        let Response::Failure(failure) = client
            .request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(9),
                call: Method::DeliveryRead {
                    target,
                    locator: crate::observation::content::CanonicalContentLocator::ByMessageId {
                        message_id: crate::runtime::identity::MessageId::new("invented"),
                    },
                    delivery_index: 0,
                },
            })
            .await
        else {
            panic!("no delivery exists")
        };
        failure.error.data.unwrap()
    }
    bounded(async {
        const DELIVERY: &str = "delivery-secret-000000000000000000000000000000000000";
        const PRODUCT_HOST: &str = "product-host-secret-0000000000000000000000000000000000";
        let f = Fixture::with_session_count(None, 1).await;
        f.host
            .bind_delivery_access(Some(websocket::Credential::new(DELIVERY.into()).unwrap()));
        f.host
            .bind_product_host(Some(websocket::Credential::new(PRODUCT_HOST.into()).unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let stop = CancellationToken::new();
        let serving = tokio::spawn(websocket::serve(
            listener,
            f.host.clone(),
            websocket::Credential::new(driver::TOKEN.into()).unwrap(),
            stop.clone(),
        ));
        let token = driver::TOKEN;
        for offered in [
            format!("rustx.app-server.v57, rustx-delivery-access.{DELIVERY}"),
            format!("rustx.app-server.v57, rustx-token.{token}, rustx-delivery-access.{PRODUCT_HOST}"),
            format!("rustx.app-server.v57, rustx-token.{token}, rustx-delivery-access.{token}"),
            format!("rustx.app-server.v57, rustx-token.{token}, rustx-delivery-access.wrong"),
            format!("rustx.app-server.v57, rustx-token.{token}, rustx-product-host.{PRODUCT_HOST}, rustx-delivery-access."),
        ] {
            assert!(driver::try_socket(&url, &offered).await.is_err(), "{offered}");
        }
        let unauthorized = ErrorData::SessionFileRead {
            reason: crate::tools::session_files::SessionFileReadFailure::Unauthorized,
        };
        // A Product Host secret offered on the ordinary lane is not delivery access.
        let ordinary = driver::websocket_offering(
            &url,
            &format!("rustx.app-server.v57, rustx-token.{token}, rustx-product-host.{PRODUCT_HOST}"),
        )
        .await;
        assert!(!delivery_access(&ordinary).await);
        let target = attach(&ordinary, &f).await;
        assert_eq!(read(&ordinary, target.clone()).await, unauthorized);
        call(&ordinary, 2, Method::SessionDetach { target }).await;
        ordinary.close().await;
        let granted = driver::websocket_offering(
            &url,
            &format!("rustx.app-server.v57, rustx-token.{token}, rustx-delivery-access.{DELIVERY}"),
        )
        .await;
        assert!(delivery_access(&granted).await);
        let target = attach(&granted, &f).await;
        assert_eq!(
            read(&granted, target.clone()).await,
            ErrorData::SessionFileRead {
                reason: crate::tools::session_files::SessionFileReadFailure::Unavailable,
            },
            "authorized lookup of an invented coordinate finds no delivery"
        );
        f.host.bind_delivery_access(None);
        assert_eq!(read(&granted, target).await, unauthorized, "removal revokes live grants");
        assert!(
            driver::try_socket(
                &url,
                &format!("rustx.app-server.v57, rustx-token.{token}, rustx-delivery-access.{DELIVERY}"),
            )
            .await
            .is_err()
        );
        granted.close().await;
        stop.cancel();
        serving.await.unwrap().unwrap();
        f.close().await;
    })
    .await;
}

/// The server's side of a stream with test-controlled acceptance, like a pipe
/// or socket buffer the peer drains only when the test says so. With no
/// budget, `poll_write` takes nothing, records what it was offered and
/// returns `Pending`; a grant wakes the writer. Reads pass through. Either
/// direction can also be held (see [`Hold`]).
struct Valve<S> {
    inner: S,
    control: Arc<ValveControl>,
}
struct ValveControl {
    /// Bytes still accepted, and the writer to wake on a grant.
    state: std::sync::Mutex<(usize, Option<std::task::Waker>)>,
    /// Every refused offer, in order.
    refused: tokio::sync::watch::Sender<Vec<Vec<u8>>>,
    /// Bytes the inner stream has taken.
    written: std::sync::atomic::AtomicUsize,
    reads: Hold,
    writes: Hold,
}
impl ValveControl {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: std::sync::Mutex::new((usize::MAX, None)),
            refused: tokio::sync::watch::channel(Vec::new()).0,
            written: std::sync::atomic::AtomicUsize::new(0),
            reads: Hold::new(),
            writes: Hold::new(),
        })
    }
    /// Accepts the next `bytes` bytes, then refuses again.
    fn grant(&self, bytes: usize) {
        let waker = {
            let mut state = self.state.lock().unwrap();
            state.0 = bytes;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
    /// Waits for a refused offer satisfying `matches`, and returns it.
    async fn refused(&self, matches: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let mut refused = self.refused.subscribe();
        let offers = refused
            .wait_for(|offers| offers.iter().any(|offer| matches(offer)))
            .await
            .unwrap();
        offers.iter().find(|offer| matches(offer)).unwrap().clone()
    }
    fn offered(&self, matches: impl Fn(&[u8]) -> bool) -> bool {
        self.refused.borrow().iter().any(|offer| matches(offer))
    }
    fn written(&self) -> usize {
        self.written.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Releases both holds when dropped, so a failing assertion unwinds
    /// instead of leaving a transport poll blocked.
    fn released_on_drop(self: &Arc<Self>) -> impl Drop {
        struct Released(Arc<ValveControl>);
        impl Drop for Released {
            fn drop(&mut self) {
                self.0.reads.release();
                self.0.writes.release();
            }
        }
        Released(self.clone())
    }
}

/// Which offers an armed [`Hold`] parks.
type Offers = Box<dyn Fn(&[u8]) -> bool + Send>;
/// A test-controlled point inside one transport poll. Once armed, the next
/// poll to reach it with a matching offer blocks its thread right there,
/// still holding whatever that poll holds (the delivery decision, the shared
/// stream), until the test releases it. `entered` reports that offer.
struct Hold {
    armed: std::sync::Mutex<Option<Offers>>,
    released: std::sync::Condvar,
    entered: tokio::sync::watch::Sender<Option<Vec<u8>>>,
}
impl Hold {
    fn new() -> Self {
        Self {
            armed: std::sync::Mutex::new(None),
            released: std::sync::Condvar::new(),
            entered: tokio::sync::watch::channel(None).0,
        }
    }
    fn arm(&self) {
        self.arm_for(|_| true);
    }
    fn arm_for(&self, offers: impl Fn(&[u8]) -> bool + Send + 'static) {
        self.entered.send_replace(None);
        *self.armed.lock().unwrap() = Some(Box::new(offers));
    }
    fn release(&self) {
        *self.armed.lock().unwrap() = None;
        self.released.notify_all();
    }
    fn enter(&self, offered: &[u8]) {
        let mut armed = self.armed.lock().unwrap();
        if !armed.as_ref().is_some_and(|offers| offers(offered)) {
            return;
        }
        self.entered.send_replace(Some(offered.to_vec()));
        while armed.is_some() {
            armed = self.released.wait(armed).unwrap();
        }
    }
    /// Waits until a poll is held here, and returns what it was offering.
    async fn entered(&self) -> Vec<u8> {
        self.entered
            .subscribe()
            .wait_for(Option::is_some)
            .await
            .unwrap()
            .clone()
            .unwrap()
    }
}
impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Valve<S> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let this = self.get_mut();
        this.control.reads.enter(&[]);
        std::pin::Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}
impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Valve<S> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.control.writes.enter(buf);
        let mut state = this.control.state.lock().unwrap();
        if state.0 == 0 {
            state.1 = Some(cx.waker().clone());
            this.control
                .refused
                .send_modify(|offers| offers.push(buf.to_vec()));
            return std::task::Poll::Pending;
        }
        let allowed = buf.len().min(state.0);
        let written =
            std::task::ready!(std::pin::Pin::new(&mut this.inner).poll_write(cx, &buf[..allowed]))?;
        state.0 -= written;
        this.control
            .written
            .fetch_add(written, std::sync::atomic::Ordering::SeqCst);
        std::task::Poll::Ready(Ok(written))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// A peer can consume a delimiter before the writer returns from its poll.
/// Closing acceptance joins that poll before sampling its byte accounting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valve_zero_budget_barrier_includes_physically_accepted_delimiter() {
    use std::{pin::Pin, task::Poll};
    use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

    struct AcceptedWrite {
        inner: tokio::io::DuplexStream,
        accepted: Arc<Hold>,
    }
    struct Release(Arc<Hold>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    impl AsyncWrite for AcceptedWrite {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            let count = std::task::ready!(Pin::new(&mut self.inner).poll_write(cx, bytes))?;
            if count > 0 {
                self.accepted.enter(&bytes[..count]);
            }
            Poll::Ready(Ok(count))
        }
        fn poll_flush(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    bounded(async {
        let control = ValveControl::new();
        let accepted = Arc::new(Hold::new());
        let _release = Release(accepted.clone());
        accepted.arm_for(|offered| offered == b"\n");
        let (inner, mut peer) = tokio::io::duplex(64);
        let mut writer = Valve {
            inner: AcceptedWrite {
                inner,
                accepted: accepted.clone(),
            },
            control: control.clone(),
        };
        let writing = tokio::spawn(async move {
            writer.write_all(b"warmup").await.unwrap();
            writer.write_all(b"\n").await.unwrap();
            writer
        });
        assert_eq!(accepted.entered().await, b"\n");
        let mut record = [0; 7];
        peer.read_exact(&mut record).await.unwrap();
        assert_eq!(&record, b"warmup\n");
        assert_eq!(
            control.written(),
            record.len() - 1,
            "peer consumption precedes delimiter accounting"
        );
        assert!(matches!(
            control.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let closing = {
            let control = control.clone();
            tokio::task::spawn_blocking(move || {
                control.grant(0);
                control.written()
            })
        };
        accepted.release();
        let accepted_before = closing.await.unwrap();
        assert_eq!(accepted_before, record.len());
        let mut writer = writing.await.unwrap();
        std::future::poll_fn(|cx| {
            assert!(Pin::new(&mut writer).poll_write(cx, b"next\n").is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(control.refused(|_| true).await, b"next\n");
        assert_eq!(
            control.written(),
            accepted_before,
            "no next-record bytes accepted"
        );
    })
    .await;
}

fn server_info(id: i64) -> Request {
    Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(id),
        call: Method::ServerInfo {},
    }
}
/// Whether `record` is the complete JSON-RPC response for `id`, and if so
/// whether it is a success.
fn response(record: &[u8], id: i64) -> Option<bool> {
    let value: serde_json::Value = serde_json::from_slice(record).ok()?;
    (value.get("id")? == id).then(|| value.get("result").is_some())
}
/// The payload of one unmasked server WebSocket frame.
fn frame_payload(frame: &[u8]) -> &[u8] {
    let header = match frame.get(1).map(|length| length & 0x7f) {
        Some(126) => 4,
        Some(127) => 10,
        _ => 2,
    };
    frame.get(header..).unwrap_or_default()
}

/// A driver whose every received record carrying an id is also kept, in
/// order, so a test can count terminal responses per id.
type Received = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;
fn recording<S>(
    incoming: S,
) -> (
    impl futures_util::Stream<Item = Result<String, String>>,
    Received,
)
where
    S: futures_util::Stream<Item = Result<String, String>>,
{
    use futures_util::StreamExt;
    let received = Received::default();
    let kept = received.clone();
    let incoming = incoming.inspect(move |record| {
        let Ok(record) = record else { return };
        let value: serde_json::Value = serde_json::from_str(record).unwrap();
        if value.get("id").is_some() {
            kept.lock().unwrap().push(value);
        }
    });
    (incoming, received)
}
#[track_caller]
fn answered_once(received: &Received, id: i64, times: usize) {
    let received = received.lock().unwrap();
    let ids: Vec<_> = received.iter().map(|value| value["id"].clone()).collect();
    assert_eq!(
        ids.iter().filter(|seen| **seen == id).count(),
        times,
        "terminal responses for {id}"
    );
    let unique: std::collections::BTreeSet<_> = ids.iter().map(ToString::to_string).collect();
    assert_eq!(unique.len(), ids.len(), "no id is answered twice: {ids:?}");
}

/// The delivery publication linearization point is the transport's acceptance
/// of the response's first bytes, over a real stdio transport whose write
/// readiness the test controls.
///
/// - queued: the response is produced while the writer is stuck on an
///   earlier record and never offered; a cancel wins.
/// - cancel / credential / detach / close: the produced success is dequeued
///   and offered while the pipe takes nothing; the revocation wins, and the
///   same id's typed failure is what the pipe finally takes.
/// - published: the pipe takes one byte of the success, which fixes it; a
///   later cancel is refused, a later revocation retracts nothing, and the
///   whole success arrives.
/// - shutdown: transport shutdown while the success is offered transmits
///   nothing for it, and the operation ends with its transport.
///
/// Every case: one terminal response per id (none after shutdown), unrelated
/// requests answered, every native read permit returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_publication_linearizes_at_the_transports_first_accepted_byte() {
    use super::protocol::delivery::{
        DELIVERED, cancel_delivery, committed_delivery, delivered_bytes, delivery_request,
        failed_with,
    };
    use crate::tools::session_files::{SESSION_FILE_MAX_READS, SessionFileReadFailure};
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("present"), 1).await;
        let tool = committed_delivery(&f).await;
        let probe = f.host.file_read_probe();
        let permits = f.host.file_reads();
        let unauthorized = ErrorData::SessionFileRead {
            reason: SessionFileReadFailure::Unauthorized,
        };
        let success_30 = |offer: &[u8]| response(offer, 30) == Some(true);
        for case in [
            "queued",
            "cancel",
            "credential",
            "detach",
            "close",
            "published",
            "shutdown",
        ] {
            let connection = Arc::new(AppServerConnection::with_delivery_access(
                f.host.clone(),
                CancellationToken::new(),
            ));
            let valve = ValveControl::new();
            let shutdown = CancellationToken::new();
            let (client, server) = tokio::io::duplex(1 << 20);
            let (reader, writer) = tokio::io::split(server);
            let serving = tokio::spawn(stdio::serve(
                connection.clone(),
                reader,
                Valve {
                    inner: writer,
                    control: valve.clone(),
                },
                shutdown.clone(),
            ));
            let (reader, writer) = tokio::io::split(client);
            let lines = driver::lines(tokio::io::BufReader::new(reader));
            let (incoming, received) = recording(lines);
            let client = driver::Driver::new(incoming, driver::jsonl_sink(writer));
            initialize(&client).await;
            let target = attach(&client, &f).await;
            probe.completed.send_replace(None);
            let read = || delivery_request(30, &target, &tool, false);

            if case == "shutdown" {
                valve.grant(0);
                {
                    let request = client.try_request(read());
                    tokio::pin!(request);
                    tokio::select! {
                        biased;
                        outcome = &mut request => panic!("no outcome before shutdown: {outcome:?}"),
                        _ = valve.refused(success_30) => {}
                    }
                    shutdown.cancel();
                    serving.await.unwrap().unwrap();
                    // The carrier's end fails the waiter instead of answering it.
                    assert_eq!(
                        request.await,
                        Err(driver::TransportEnded("end of input".into()))
                    );
                }
                answered_once(&received, 30, 0);
                // The transport's end closed the connection; its undecided
                // operation ended with it.
                let Response::Failure(closed) = connection
                    .handle_request(Request {
                        jsonrpc: JsonRpcVersion::V2,
                        id: RequestId::Integer(33),
                        call: Method::DeliveryCancel {
                            request_id: RequestId::Integer(30),
                        },
                    })
                    .await
                else {
                    panic!("a closed connection admits nothing")
                };
                assert_eq!(closed.error.data, Some(ErrorData::StaleAttachment));
                assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
                client.close().await;
                connection.close();
                continue;
            }

            let (outcome, expected) = match case {
                "queued" => {
                    // The writer is stuck offering an earlier record.
                    valve.grant(0);
                    let (earlier, outcome) = tokio::join!(client.request(server_info(29)), async {
                        valve
                            .refused(|offer| response(offer, 29) == Some(true))
                            .await;
                        let (outcome, ()) = tokio::join!(client.request(read()), async {
                            probe
                                .completed
                                .subscribe()
                                .wait_for(Option::is_some)
                                .await
                                .unwrap();
                            assert_eq!(*probe.completed.borrow(), Some(true), "produced");
                            assert!(
                                !valve.offered(|offer| response(offer, 30).is_some()),
                                "never offered to the transport"
                            );
                            assert!(cancel_delivery(&connection, 30).await, "the cancel wins");
                            valve.grant(usize::MAX);
                        });
                        outcome
                    });
                    assert!(matches!(earlier, Response::Success(_)));
                    (outcome, Some(ErrorData::DeliveryCancelled))
                }
                "published" => {
                    valve.grant(0);
                    let (outcome, ()) = tokio::join!(client.request(read()), async {
                        let offer = valve.refused(success_30).await;
                        // One byte: the writer offers again, decides, and the
                        // pipe takes one byte. Its next offer is exactly the rest.
                        valve.grant(1);
                        valve.refused(|rest| rest == &offer[1..]).await;
                        assert!(
                            !cancel_delivery(&connection, 30).await,
                            "after the first accepted byte the response stands"
                        );
                        connection.revoke_delivery_access();
                        valve.grant(usize::MAX);
                    });
                    (outcome, None)
                }
                revocation => {
                    valve.grant(0);
                    let (outcome, expected) = tokio::join!(client.request(read()), async {
                        valve.refused(success_30).await;
                        assert_eq!(*probe.completed.borrow(), Some(true), "produced");
                        let expected = match revocation {
                            "cancel" => {
                                assert!(cancel_delivery(&connection, 30).await, "the cancel wins");
                                ErrorData::DeliveryCancelled
                            }
                            "credential" => {
                                connection.revoke_delivery_access();
                                unauthorized.clone()
                            }
                            "detach" => {
                                connection
                                    .handle_request(Request {
                                        jsonrpc: JsonRpcVersion::V2,
                                        id: RequestId::Integer(31),
                                        call: Method::SessionDetach {
                                            target: target.clone(),
                                        },
                                    })
                                    .await;
                                ErrorData::StaleAttachment
                            }
                            _ => {
                                connection.close();
                                unauthorized.clone()
                            }
                        };
                        valve.grant(usize::MAX);
                        expected
                    });
                    (outcome, Some(expected))
                }
            };
            match expected {
                Some(expected) => failed_with(&outcome, &expected),
                None => assert_eq!(delivered_bytes(&outcome), DELIVERED),
            }
            if case != "close" {
                assert!(matches!(
                    client.request(server_info(32)).await,
                    Response::Success(_)
                ));
            }
            answered_once(&received, 30, 1);
            assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
            client.close().await;
            serving.await.unwrap().unwrap();
            connection.close();
        }
        f.close().await;
    }))
    .await;
}

/// A cancel or revocation that overlaps a delivery response's hand-off, after
/// its decision and before the transport's acceptance, waits for that
/// acceptance and is ordered after it: the success stands, and the
/// revocation governs every later publication. Real stdio transport; the
/// pipe's acceptance is held inside `poll_write`, with the decided success in
/// hand, while the revocation runs on another thread.
///
/// - credential: the connection's delivery authority is revoked;
/// - detach: the exact attachment is detached;
/// - close: the connection closes while its publication is in progress;
/// - cancel: `delivery/cancel` for the same id is refused, and a sibling
///   delivery request is unaffected.
///
/// Each case establishes the interleaving rather than timing it: the pipe was
/// offered the success (so the decision was made) and is held; the
/// revocation then reports that it found that hand-off in progress and is
/// waiting, and has not completed; only the release completes both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_revocation_overlapping_a_hand_off_is_ordered_after_its_acceptance() {
    use super::protocol::delivery::{
        DELIVERED, cancel_delivery, committed_delivery, delivered_bytes, delivery_request,
        failed_with,
    };
    use crate::tools::session_files::{SESSION_FILE_MAX_READS, SessionFileReadFailure};
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("present"), 1).await;
        let tool = committed_delivery(&f).await;
        let probe = f.host.file_read_probe();
        let permits = f.host.file_reads();
        let revocations = f.host.revocations().clone();
        let success_30 = |offer: &[u8]| response(offer, 30) == Some(true);
        for case in ["credential", "detach", "close", "cancel"] {
            let connection = Arc::new(AppServerConnection::with_delivery_access(
                f.host.clone(),
                CancellationToken::new(),
            ));
            let valve = ValveControl::new();
            let _released = valve.released_on_drop();
            let (client, server) = tokio::io::duplex(1 << 20);
            let (reader, writer) = tokio::io::split(server);
            let serving = tokio::spawn(stdio::serve(
                connection.clone(),
                reader,
                Valve {
                    inner: writer,
                    control: valve.clone(),
                },
                CancellationToken::new(),
            ));
            let (reader, writer) = tokio::io::split(client);
            let lines = driver::lines(tokio::io::BufReader::new(reader));
            let (incoming, received) = recording(lines);
            let client = driver::Driver::new(incoming, driver::jsonl_sink(writer));
            initialize(&client).await;
            let target = attach(&client, &f).await;
            revocations.waited.send_replace(0);
            probe.cancel_waited.send_replace(false);
            valve.grant(0);
            let (outcome, ()) = tokio::join!(
                client.request(delivery_request(30, &target, &tool, false)),
                async {
                    // Undecided while the pipe takes nothing.
                    valve.refused(success_30).await;
                    // The next offer decides the success, and its acceptance
                    // is held inside the pipe.
                    valve.writes.arm();
                    valve.grant(usize::MAX);
                    assert!(
                        success_30(&valve.writes.entered().await),
                        "decided as produced"
                    );
                    let handle = tokio::runtime::Handle::current();
                    let revoking = tokio::task::spawn_blocking({
                        let (connection, target) = (connection.clone(), target.clone());
                        move || match case {
                            "credential" => {
                                connection.revoke_delivery_access();
                                None
                            }
                            "detach" => {
                                handle.block_on(connection.handle_request(Request {
                                    jsonrpc: JsonRpcVersion::V2,
                                    id: RequestId::Integer(31),
                                    call: Method::SessionDetach { target },
                                }));
                                None
                            }
                            "close" => {
                                connection.close();
                                None
                            }
                            _ => Some(handle.block_on(cancel_delivery(&connection, 30))),
                        }
                    });
                    tokio::pin!(revoking);
                    let waited = async {
                        if case == "cancel" {
                            let _ = probe
                                .cancel_waited
                                .subscribe()
                                .wait_for(|waited| *waited)
                                .await;
                        } else {
                            let _ = revocations
                                .waited
                                .subscribe()
                                .wait_for(|waited| *waited > 0)
                                .await;
                        }
                    };
                    tokio::select! {
                        biased;
                        () = waited => {}
                        _ = &mut revoking => panic!(
                            "{case} completed between the decision and the transport's acceptance"
                        ),
                    }
                    valve.writes.release();
                    assert_eq!(
                        revoking.await.unwrap(),
                        (case == "cancel").then_some(false),
                        "a cancel ordered after the acceptance is refused"
                    );
                }
            );
            assert_eq!(
                delivered_bytes(&outcome),
                DELIVERED,
                "accepted before the {case}: the success stands"
            );
            // The revocation governs every later publication.
            let later = || client.request(delivery_request(34, &target, &tool, false));
            match case {
                "credential" => failed_with(
                    &later().await,
                    &ErrorData::SessionFileRead {
                        reason: SessionFileReadFailure::Unauthorized,
                    },
                ),
                "detach" => failed_with(&later().await, &ErrorData::StaleAttachment),
                "cancel" => assert_eq!(
                    delivered_bytes(&later().await),
                    DELIVERED,
                    "a sibling request is not cancelled"
                ),
                _ => {}
            }
            if case != "close" {
                assert!(matches!(
                    client.request(server_info(32)).await,
                    Response::Success(_)
                ));
                answered_once(&received, 34, 1);
            }
            answered_once(&received, 30, 1);
            assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
            client.close().await;
            serving.await.unwrap().unwrap();
            connection.close();
        }
        f.close().await;
    }))
    .await;
}

/// The WebSocket writer stages nothing outside tungstenite. While the shared
/// stream is held by the reader (blocked inside a socket read), the writer's
/// hand-off of a produced, queued success waits for the stream without
/// deciding; a revocation completes meanwhile; once the reader releases the
/// stream the writer decides, and what tungstenite takes and the peer
/// receives is the same id's typed failure, never the stale success.
/// `SplitSink` instead parked the success in its own slot as if accepted and
/// forwarded it to tungstenite after the revocation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_writer_stages_nothing_while_the_reader_holds_the_stream() {
    use super::protocol::delivery::{committed_delivery, delivery_request, failed_with};
    use crate::tools::session_files::{SESSION_FILE_MAX_READS, SessionFileReadFailure};
    use futures_util::StreamExt;
    use tokio_tungstenite::{
        WebSocketStream,
        tungstenite::{Message, protocol::Role},
    };
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("present"), 1).await;
        let tool = committed_delivery(&f).await;
        let permits = f.host.file_reads();
        let connection = Arc::new(AppServerConnection::with_delivery_access(
            f.host.clone(),
            CancellationToken::new(),
        ));
        let direct = app_server_conformance::DirectDriver(&connection);
        initialize(&direct).await;
        let target = attach(&direct, &f).await;
        // 1. A delivery success, produced and queued for the writer.
        let reply = connection
            .reply(delivery_request(30, &target, &tool, false))
            .await;
        assert!(matches!(reply.produced(), Response::Success(_)), "produced");
        let mut outbound = transport::Outbound::reply(reply).unwrap();
        let valve = ValveControl::new();
        let _released = valve.released_on_drop();
        let (peer, server) = tokio::io::duplex(1 << 20);
        let socket = Arc::new(websocket::Socket::new(
            WebSocketStream::from_raw_socket(
                Valve {
                    inner: server,
                    control: valve.clone(),
                },
                Role::Server,
                None,
            )
            .await,
        ));
        let mut peer = WebSocketStream::from_raw_socket(peer, Role::Client, None).await;
        let handle = tokio::runtime::Handle::current();
        // The reader holds the shared stream inside a socket read.
        valve.reads.arm();
        let reader = tokio::task::spawn_blocking({
            let (socket, handle) = (socket.clone(), handle.clone());
            move || {
                handle.block_on(std::future::poll_fn(|cx| {
                    let _ = socket.poll_next(cx);
                    std::task::Poll::Ready(())
                }));
            }
        });
        valve.reads.entered().await;
        // 2-3. The writer offers the success and waits for the stream.
        let writer = tokio::task::spawn_blocking({
            let socket = socket.clone();
            move || {
                handle.block_on(std::future::poll_fn(|cx| {
                    socket.poll_hand_off(cx, &mut outbound)
                }))
            }
        });
        tokio::pin!(writer);
        let mut waited = socket.waited.subscribe();
        tokio::select! {
            biased;
            _ = waited.wait_for(|waited| *waited) => {}
            _ = &mut writer => panic!("a hand-off finished while the reader held the stream"),
        }
        assert_eq!(valve.written(), 0, "tungstenite has taken nothing");
        // 4. A revocation completes.
        connection.revoke_delivery_access();
        // 5-6. The reader releases the stream; the writer resumes.
        valve.reads.release();
        reader.await.unwrap();
        let handed = writer.await.unwrap().unwrap();
        assert_eq!(
            response(handed.as_bytes(), 30),
            Some(false),
            "decided only after the revocation"
        );
        socket.flush().await.unwrap();
        // 7. The peer receives the same id's typed failure, never the success.
        let Some(Ok(Message::Text(text))) = peer.next().await else {
            panic!("one frame")
        };
        failed_with(
            &serde_json::from_str(&text).unwrap(),
            &ErrorData::SessionFileRead {
                reason: SessionFileReadFailure::Unauthorized,
            },
        );
        assert_eq!(response(text.as_bytes(), 30), Some(false), "the same id");
        assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
        connection.close();
        f.close().await;
    }))
    .await;
}

/// Over WebSocket the publication point is tungstenite's acceptance of the
/// frame, which `websocket::connection` reaches only once every earlier frame
/// has gone to the socket. Behind an earlier frame the socket has not taken,
/// a delivery response stays undecided and a credential revocation wins.
/// Once tungstenite has taken the frame, even into its own buffer because the
/// socket takes nothing, the frame is published and a revocation does not
/// retract it: that is the documented WebSocket limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_delivery_publication_is_decided_when_tungstenite_takes_the_frame() {
    use super::protocol::delivery::{
        DELIVERED, committed_delivery, delivered_bytes, delivery_request, failed_with,
    };
    use crate::tools::session_files::{SESSION_FILE_MAX_READS, SessionFileReadFailure};
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    bounded(Box::pin(async {
        const DELIVERY: &str = "delivery-secret-000000000000000000000000000000000000";
        let f = Fixture::with_session_count(Some("present"), 1).await;
        let tool = committed_delivery(&f).await;
        let probe = f.host.file_read_probe();
        let permits = f.host.file_reads();
        let frame_for = |id: i64, success: bool| {
            move |frame: &[u8]| response(frame_payload(frame), id) == Some(success)
        };
        for behind_unsent_frame in [true, false] {
            f.host
                .bind_delivery_access(Some(websocket::Credential::new(DELIVERY.into()).unwrap()));
            let valve = ValveControl::new();
            let (client, server) = tokio::io::duplex(1 << 20);
            let serving = tokio::spawn(websocket::connection(
                Valve {
                    inner: server,
                    control: valve.clone(),
                },
                f.host.clone(),
                websocket::Credential::new(driver::TOKEN.into()).unwrap(),
                CancellationToken::new(),
            ));
            let mut request = "ws://localhost/".into_client_request().unwrap();
            request.headers_mut().insert(
                "sec-websocket-protocol",
                format!(
                    "rustx.app-server.v57, rustx-token.{}, rustx-delivery-access.{DELIVERY}",
                    driver::TOKEN
                )
                .parse()
                .unwrap(),
            );
            let (socket, _) = tokio_tungstenite::client_async(request, client)
                .await
                .unwrap();
            let (sink, stream) = socket.split();
            let texts = driver::frames(stream);
            let (incoming, received) = recording(texts);
            let client = driver::Driver::new(incoming, driver::text_sink(sink));
            initialize(&client).await;
            let target = attach(&client, &f).await;
            probe.completed.send_replace(None);
            let read = || delivery_request(30, &target, &tool, false);
            valve.grant(0);
            if behind_unsent_frame {
                let (earlier, outcome) = tokio::join!(client.request(server_info(29)), async {
                    valve.refused(frame_for(29, true)).await;
                    let (outcome, ()) = tokio::join!(client.request(read()), async {
                        probe
                            .completed
                            .subscribe()
                            .wait_for(Option::is_some)
                            .await
                            .unwrap();
                        assert_eq!(*probe.completed.borrow(), Some(true), "produced");
                        assert!(
                            !valve.offered(|frame| response(frame_payload(frame), 30).is_some()),
                            "not yet taken by tungstenite"
                        );
                        f.host.bind_delivery_access(None);
                        valve.grant(usize::MAX);
                    });
                    outcome
                });
                assert!(matches!(earlier, Response::Success(_)));
                failed_with(
                    &outcome,
                    &ErrorData::SessionFileRead {
                        reason: SessionFileReadFailure::Unauthorized,
                    },
                );
            } else {
                let (outcome, ()) = tokio::join!(client.request(read()), async {
                    valve.refused(frame_for(30, true)).await;
                    f.host.bind_delivery_access(None);
                    valve.grant(usize::MAX);
                });
                assert_eq!(
                    delivered_bytes(&outcome),
                    DELIVERED,
                    "taken by tungstenite, the frame is not retracted"
                );
            }
            assert!(matches!(
                client.request(server_info(32)).await,
                Response::Success(_)
            ));
            answered_once(&received, 30, 1);
            assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
            client.close().await;
            let _ = serving.await.unwrap();
        }
        f.close().await;
    }))
    .await;
}

const ROTATION_A: &str = "rotation-secret-a-0000000000000000000000000000000000000";
const ROTATION_B: &str = "rotation-secret-b-0000000000000000000000000000000000000";
const ROTATION_C: &str = "rotation-secret-c-0000000000000000000000000000000000000";
fn rotation_credential(secret: &str) -> websocket::Credential {
    websocket::Credential::new(secret.into()).unwrap()
}
/// Authenticates `secret` as a delivery credential through the handshake's
/// own authority path: a fresh token, or `None` when refused.
fn delivery_token(f: &Fixture, secret: &str) -> Option<CancellationToken> {
    let offered = format!(
        "{}{secret}",
        crate::app_server::delivery_access::CREDENTIAL_PREFIX
    );
    f.host
        .authenticate_delivery_access(&[offered.as_str()])
        .ok()
        .flatten()
}
/// The same, for the Product Host credential.
fn product_host_token(f: &Fixture, secret: &str) -> Option<CancellationToken> {
    let offered = format!("rustx-product-host.{secret}");
    f.host.authenticate_product_host(&[
        crate::app_server::product_host::SUBPROTOCOL,
        offered.as_str(),
    ])
}
/// Waits until `count` revocations found a publication in progress, failing
/// if any of `revoking` completed first.
async fn revocations_waiting(
    f: &Fixture,
    count: usize,
    revoking: &mut [std::pin::Pin<&mut tokio::task::JoinHandle<()>>],
) {
    let mut waited = f.host.revocations().waited.subscribe();
    let completed = futures_util::future::select_all(revoking.iter_mut());
    tokio::select! {
        biased;
        _ = waited.wait_for(|waited| *waited >= count) => {}
        _ = completed => panic!("a rotation completed while a publication was between its decision and its acceptance"),
    }
}
/// An ordinary App Server WebSocket connection over a [`Valve`], admitted
/// with `credential` as its delivery access, initialized and attached.
async fn delivery_websocket(
    f: &Fixture,
    credential: &str,
    valve: &Arc<ValveControl>,
) -> (
    driver::Driver,
    Received,
    tokio::task::JoinHandle<io::Result<()>>,
    AttachmentTarget,
) {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (client, server) = tokio::io::duplex(1 << 20);
    let serving = tokio::spawn(websocket::connection(
        Valve {
            inner: server,
            control: valve.clone(),
        },
        f.host.clone(),
        websocket::Credential::new(driver::TOKEN.into()).unwrap(),
        CancellationToken::new(),
    ));
    let mut request = "ws://localhost/".into_client_request().unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!(
            "rustx.app-server.v57, rustx-token.{}, rustx-delivery-access.{credential}",
            driver::TOKEN
        )
        .parse()
        .unwrap(),
    );
    let (socket, _) = tokio_tungstenite::client_async(request, client)
        .await
        .unwrap();
    let (sink, stream) = socket.split();
    let texts = driver::frames(stream);
    let (incoming, received) = recording(texts);
    let client = driver::Driver::new(incoming, driver::text_sink(sink));
    initialize(&client).await;
    let target = attach(&client, f).await;
    (client, received, serving, target)
}

/// Replacing or removing the delivery credential is one authority
/// transition, ordered against publication by the revocation order.
///
/// An A-admitted WebSocket connection's success for 30 is decided and held
/// inside tungstenite's acceptance. A rotation to B (or a removal) then
/// reports that it waits for that publication and has not completed. While it
/// waits, B does not authenticate, and A still does (a token minted then is
/// revoked by the rotation). After the release the success stands, the
/// rotation completes, A's tokens (including the connection's) are cancelled,
/// only B authenticates, and the A connection's next delivery is
/// `unauthorized`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delivery_credential_rotation_is_one_transition_ordered_against_publication() {
    use super::protocol::delivery::{
        DELIVERED, committed_delivery, delivered_bytes, delivery_request, failed_with,
    };
    use crate::tools::session_files::{SESSION_FILE_MAX_READS, SessionFileReadFailure};
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("present"), 1).await;
        let tool = committed_delivery(&f).await;
        let permits = f.host.file_reads();
        for next in [Some(ROTATION_B), None] {
            f.host
                .bind_delivery_access(Some(rotation_credential(ROTATION_A)));
            let valve = ValveControl::new();
            let _released = valve.released_on_drop();
            let (client, received, serving, target) =
                delivery_websocket(&f, ROTATION_A, &valve).await;
            f.host.revocations().waited.send_replace(0);
            valve
                .writes
                .arm_for(|frame| response(frame_payload(frame), 30) == Some(true));
            let (outcome, ()) = tokio::join!(
                client.request(delivery_request(30, &target, &tool, false)),
                async {
                    valve.writes.entered().await;
                    let rotating = tokio::task::spawn_blocking({
                        let host = f.host.clone();
                        move || host.bind_delivery_access(next.map(rotation_credential))
                    });
                    tokio::pin!(rotating);
                    revocations_waiting(&f, 1, &mut [rotating.as_mut()]).await;
                    // The previous credential is not revoked yet, so the
                    // next one is not observable either.
                    assert!(
                        delivery_token(&f, ROTATION_B).is_none(),
                        "B observable before A is revoked"
                    );
                    let early = delivery_token(&f, ROTATION_A).expect("A, before the rotation");
                    valve.writes.release();
                    rotating.await.unwrap();
                    assert!(
                        early.is_cancelled(),
                        "a token minted before the rotation is revoked by it"
                    );
                }
            );
            assert_eq!(
                delivered_bytes(&outcome),
                DELIVERED,
                "accepted before the rotation: the success stands"
            );
            assert!(delivery_token(&f, ROTATION_A).is_none(), "A is revoked");
            match next {
                Some(_) => assert!(
                    !delivery_token(&f, ROTATION_B)
                        .expect("B after the rotation")
                        .is_cancelled()
                ),
                None => assert!(delivery_token(&f, ROTATION_B).is_none()),
            }
            failed_with(
                &client
                    .request(delivery_request(34, &target, &tool, false))
                    .await,
                &ErrorData::SessionFileRead {
                    reason: SessionFileReadFailure::Unauthorized,
                },
            );
            assert!(matches!(
                client.request(server_info(32)).await,
                Response::Success(_)
            ));
            answered_once(&received, 30, 1);
            answered_once(&received, 34, 1);
            assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
            client.close().await;
            let _ = serving.await.unwrap();
        }
        f.close().await;
    }))
    .await;
}

/// Product Host credential rotation follows the same transition. A real
/// Product Host socket, admitted with A, reads a committed delivery; its
/// success is decided and held inside tungstenite's acceptance. The rotation
/// to B waits; meanwhile B does not authenticate and A does. After the
/// release the success stands, A (and every socket token it minted) is
/// revoked, and only B authenticates. The unrelated delivery-access
/// credential and an ordinary connection keep their authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn product_host_credential_rotation_is_one_transition_ordered_against_publication() {
    use super::protocol::delivery::{DELIVERED, committed_delivery};
    use crate::tools::session_files::SESSION_FILE_MAX_READS;
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("present"), 1).await;
        let tool = committed_delivery(&f).await;
        let permits = f.host.file_reads();
        f.host
            .bind_product_host(Some(rotation_credential(ROTATION_A)));
        f.host
            .bind_delivery_access(Some(rotation_credential(ROTATION_C)));
        let unrelated = delivery_token(&f, ROTATION_C).unwrap();
        let browser = AppServerConnection::new(f.host.clone());
        let direct = app_server_conformance::DirectDriver(&browser);
        initialize(&direct).await;
        let target = attach(&direct, &f).await;

        let valve = ValveControl::new();
        let _released = valve.released_on_drop();
        let (client, server) = tokio::io::duplex(1 << 20);
        let serving = tokio::spawn(websocket::connection(
            Valve {
                inner: server,
                control: valve.clone(),
            },
            f.host.clone(),
            websocket::Credential::new(driver::TOKEN.into()).unwrap(),
            CancellationToken::new(),
        ));
        let mut request = "ws://localhost/product-host/file-read"
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            format!("rustx.product-host.file-read.v5, rustx-product-host.{ROTATION_A}")
                .parse()
                .unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::client_async(request, client)
            .await
            .unwrap();
        f.host.revocations().waited.send_replace(0);
        valve
            .writes
            .arm_for(|frame| frame_payload(frame) == DELIVERED);
        socket
            .send(Message::Text(
                serde_json::to_string(&crate::app_server::product_host::FileRead {
                    read: crate::tools::session_files::preview::ReadOptions::Bytes {
                        max_bytes: crate::tools::session_files::preview::FILE_BYTES,
                        range: None,
                    },
                    target: target.clone(),
                    source: crate::app_server::product_host::ReadSource::SessionFile {
                        agent_id: None,
                        locator:
                            crate::observation::content::CanonicalContentLocator::ByMessageId {
                                message_id: tool,
                            },
                        delivery_index: 0,
                    },
                    roots: vec![f.workspaces[0].clone()],
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        let metadata = socket.next().await.unwrap().unwrap();
        let metadata: serde_json::Value =
            serde_json::from_str(metadata.to_text().unwrap()).unwrap();
        assert_eq!(
            metadata["result"]["length"].as_u64(),
            Some(DELIVERED.len() as u64)
        );
        let (frame, ()) = tokio::join!(socket.next(), async {
            valve.writes.entered().await;
            let rotating = tokio::task::spawn_blocking({
                let host = f.host.clone();
                move || host.bind_product_host(Some(rotation_credential(ROTATION_B)))
            });
            tokio::pin!(rotating);
            revocations_waiting(&f, 1, &mut [rotating.as_mut()]).await;
            assert!(
                product_host_token(&f, ROTATION_B).is_none(),
                "B observable before A is revoked"
            );
            let early = product_host_token(&f, ROTATION_A).expect("A, before the rotation");
            valve.writes.release();
            rotating.await.unwrap();
            assert!(early.is_cancelled(), "revoked by the rotation");
        });
        let Some(Ok(Message::Binary(data))) = frame else {
            panic!("the binary body accepted before rotation")
        };
        assert_eq!(data.as_ref(), DELIVERED);
        assert!(product_host_token(&f, ROTATION_A).is_none(), "A is revoked");
        assert!(!product_host_token(&f, ROTATION_B).unwrap().is_cancelled());
        // Unrelated authority is untouched.
        assert!(!unrelated.is_cancelled());
        assert!(delivery_token(&f, ROTATION_C).is_some());
        assert!(matches!(
            browser.handle_request(server_info(40)).await,
            Response::Success(_)
        ));
        drop(socket);
        let _ = serving.await.unwrap();
        assert_eq!(permits.available_permits(), SESSION_FILE_MAX_READS);
        browser.close();
        f.close().await;
    }))
    .await;
}

/// Overlapping rotations take one total order. With an A-admitted
/// publication held inside its acceptance, rotations to B, to C and to none
/// are started one after another, each observed waiting behind that
/// publication before the next starts, so all three overlap. After the
/// release each is linearized in turn: the rotation log holds exactly those
/// three installations, the final credential state is the last one's, every
/// superseded credential is refused, and the A connection's authority is
/// revoked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_delivery_credential_rotations_take_one_total_order() {
    use super::protocol::delivery::{
        DELIVERED, committed_delivery, delivered_bytes, delivery_request, failed_with,
    };
    use crate::tools::session_files::SessionFileReadFailure;
    bounded(Box::pin(async {
        let f = Fixture::with_tool(Some("present")).await;
        let tool = committed_delivery(&f).await;
        let probe = f.host.file_read_probe();
        f.host
            .bind_delivery_access(Some(rotation_credential(ROTATION_A)));
        let valve = ValveControl::new();
        let _released = valve.released_on_drop();
        let (client, _received, serving, target) = delivery_websocket(&f, ROTATION_A, &valve).await;
        probe.rotations.take();
        f.host.revocations().waited.send_replace(0);
        valve
            .writes
            .arm_for(|frame| response(frame_payload(frame), 30) == Some(true));
        let nexts = [Some(ROTATION_B), Some(ROTATION_C), None];
        let (outcome, ()) = tokio::join!(
            client.request(delivery_request(30, &target, &tool, false)),
            async {
                valve.writes.entered().await;
                let mut rotations = Vec::new();
                for (started, next) in nexts.into_iter().enumerate() {
                    rotations.push(Box::pin(tokio::task::spawn_blocking({
                        let host = f.host.clone();
                        move || host.bind_delivery_access(next.map(rotation_credential))
                    })));
                    let mut waiting: Vec<_> = rotations
                        .iter_mut()
                        .map(|rotation| rotation.as_mut())
                        .collect();
                    revocations_waiting(&f, started + 1, &mut waiting).await;
                }
                valve.writes.release();
                for rotation in rotations {
                    rotation.await.unwrap();
                }
            }
        );
        assert_eq!(
            delivered_bytes(&outcome),
            DELIVERED,
            "the publication stands"
        );
        let order = probe.rotations.take();
        let named = |installed: &Option<websocket::Credential>| {
            nexts
                .into_iter()
                .position(|next| match (next, installed) {
                    (None, None) => true,
                    (Some(secret), Some(installed)) => {
                        installed.same_secret(&rotation_credential(secret))
                    }
                    _ => false,
                })
                .expect("one of the three rotations")
        };
        let mut seen: Vec<_> = order.iter().map(named).collect();
        let last = nexts[*seen.last().expect("three linearized rotations")];
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2], "each rotation linearized exactly once");
        for secret in [ROTATION_A, ROTATION_B, ROTATION_C] {
            assert_eq!(
                delivery_token(&f, secret).is_some(),
                last == Some(secret),
                "only the last rotation's credential authenticates"
            );
        }
        failed_with(
            &client
                .request(delivery_request(34, &target, &tool, false))
                .await,
            &ErrorData::SessionFileRead {
                reason: SessionFileReadFailure::Unauthorized,
            },
        );
        client.close().await;
        let _ = serving.await.unwrap();
        f.close().await;
    }))
    .await;
}

/// Authentication racing a rotation sees one side of it. An authentication
/// with A is parked while it holds the credential slot; a rotation to B then
/// reports that it waits for that slot and has not completed. Released, the
/// authentication returns a token minted from A, the state before the
/// rotation, and the rotation then revokes it: no token escapes its grant.
/// Afterwards only B authenticates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authentication_racing_a_rotation_sees_one_side_of_it() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let probe = f.host.file_read_probe();
        f.host
            .bind_delivery_access(Some(rotation_credential(ROTATION_A)));
        probe.credential_waited.send_replace(false);
        let _gate = probe.authenticating.arm_scoped();
        let authenticating = tokio::task::spawn_blocking({
            let host = f.host.clone();
            move || {
                let offered = format!(
                    "{}{ROTATION_A}",
                    crate::app_server::delivery_access::CREDENTIAL_PREFIX
                );
                host.authenticate_delivery_access(&[offered.as_str()])
            }
        });
        tokio::task::spawn_blocking({
            let probe = probe.clone();
            move || probe.authenticating.wait_entered()
        })
        .await
        .unwrap();
        let rotating = tokio::task::spawn_blocking({
            let host = f.host.clone();
            move || host.bind_delivery_access(Some(rotation_credential(ROTATION_B)))
        });
        tokio::pin!(rotating);
        let mut waited = probe.credential_waited.subscribe();
        tokio::select! {
            biased;
            _ = waited.wait_for(|waited| *waited) => {}
            _ = &mut rotating => panic!("the rotation completed inside an authentication"),
        }
        probe.authenticating.release();
        let early = authenticating
            .await
            .unwrap()
            .unwrap()
            .expect("minted from A, before the rotation");
        rotating.await.unwrap();
        assert!(
            early.is_cancelled(),
            "no token escapes its grant's revocation"
        );
        assert!(delivery_token(&f, ROTATION_A).is_none());
        assert!(!delivery_token(&f, ROTATION_B).unwrap().is_cancelled());
        f.close().await;
    })
    .await;
}

/// The final tungstenite handoff decides cancellation and the substituted error
/// together. A real TCP WebSocket carries the selected correlated terminal reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t07_oversized_controlled_response_keeps_publication_owner_at_websocket_handoff() {
    use super::protocol::delivery::{cancel_delivery, committed_delivery, delivery_request};
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
    bounded(Box::pin(async {
        let f = Fixture::with_session_count(Some("present"), 1).await;
        let message = committed_delivery(&f).await;
        for cancel_first in [true, false] {
            let connection =
                AppServerConnection::with_delivery_access(f.host.clone(), CancellationToken::new());
            super::protocol::initialize(&connection).await;
            let MethodResult::Attached { target, .. } = super::protocol::call(
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
            let mut reply = connection
                .reply(delivery_request(30, &target, &message, false))
                .await;
            reply.force_encoding_overflow_for_test();
            let outbound = transport::Outbound::reply(reply).unwrap();
            if cancel_first {
                assert!(cancel_delivery(&connection, 30).await);
            } else {
                connection.revoke_delivery_access();
            } // a nonsensitive error keeps its reason
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (client, accepted) = tokio::join!(
                tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
                listener.accept()
            );
            let socket = websocket::Socket::new(
                tokio_tungstenite::WebSocketStream::from_raw_socket(
                    accepted.unwrap().0,
                    Role::Server,
                    Some(WebSocketConfig::default().write_buffer_size(0)),
                )
                .await,
            );
            let mut client = tokio_tungstenite::WebSocketStream::from_raw_socket(
                client.unwrap(),
                Role::Client,
                None,
            )
            .await;
            socket.send(outbound).await.unwrap();
            let bytes = client.next().await.unwrap().unwrap().into_text().unwrap();
            let Response::Failure(failure) = serde_json::from_str(&bytes).unwrap() else {
                panic!("bounded terminal failure")
            };
            assert_eq!(failure.id, Some(RequestId::Integer(30)));
            assert_eq!(
                failure.error.data,
                Some(if cancel_first {
                    ErrorData::DeliveryCancelled
                } else {
                    ErrorData::ResponseTooLarge
                })
            );
            assert!(
                !cancel_delivery(&connection, 30).await,
                "one terminal disposition at actual handoff"
            );
            connection.close();
        }
        f.close().await;
    }))
    .await;
}

/// T16: slot -> native replacement is the same boundary as final writer
/// acceptance. Pending takes zero bytes; replacement fences its next attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t16_pending_receipt_supersession_and_stale_queue_progression() {
    use crate::app_server::registration::Acceptance;
    use futures_util::StreamExt;
    use std::task::Poll;
    use tokio_tungstenite::tungstenite::protocol::Role;
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &f, 0).await;
        let MethodResult::Checkpoint { checkpoint } = super::protocol::call(
            &connection,
            80,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint")
        };
        let request = |id| Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call: Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: checkpoint.cursor,
            },
        };
        let mut r1 = transport::Outbound::reply(connection.reply(request(81)).await).unwrap();
        assert!(matches!(
            r1.poll_hand_off::<()>(|_| Poll::Pending),
            Poll::Pending
        ));
        // No event is consumed while its receipt remains unaccepted.
        let slot = connection.registration_for_test(&target);
        assert!(slot.following().is_none());
        let r2 = transport::Outbound::reply(connection.reply(request(82)).await).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, accepted) = tokio::join!(
            tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept()
        );
        let socket = websocket::Socket::new(
            tokio_tungstenite::WebSocketStream::from_raw_socket(
                accepted.unwrap().0,
                Role::Server,
                None,
            )
            .await,
        );
        let mut peer = tokio_tungstenite::WebSocketStream::from_raw_socket(
            client.unwrap(),
            Role::Client,
            None,
        )
        .await;
        socket.send(r1).await.unwrap();
        let Response::Failure(failure) =
            serde_json::from_str(&peer.next().await.unwrap().unwrap().into_text().unwrap())
                .unwrap()
        else {
            panic!("superseded")
        };
        assert_eq!(failure.id, Some(RequestId::Integer(81)));
        assert_eq!(failure.error.data, Some(ErrorData::SubscriptionSuperseded));
        socket.send(r2).await.unwrap();
        let Response::Success(_) = serde_json::from_str::<Response>(
            &peer.next().await.unwrap().unwrap().into_text().unwrap(),
        )
        .unwrap() else {
            panic!("new receipt")
        };
        let (registration, _) = slot.following().unwrap();
        let cursor = checkpoint.cursor;
        let queued = registration.clone().event(cursor);
        let (resync, _) = registration.clone().recovery();
        assert!(matches!(
            queued.hand_off("ordinary", |_| panic!(
                "RecoveryRequired fenced queued record"
            )),
            Acceptance::<()>::Discarded
        ));
        assert!(matches!(
            resync.hand_off("resync", |_| Poll::Ready(Ok(()))),
            Acceptance::Accepted(_, ())
        ));
        // A successor cannot be invalidated by its predecessor's Resync.
        let r3 = transport::Outbound::reply(connection.reply(request(83)).await).unwrap();
        assert!(matches!(
            resync.hand_off("resync", |_| panic!("old resync")),
            Acceptance::<()>::Discarded
        ));
        assert!(matches!(
            queued.hand_off("ordinary", |_| panic!("retired queue head")),
            Acceptance::<()>::Discarded
        ));
        let stale = transport::Outbound::notification((
            Notification {
                jsonrpc: JsonRpcVersion::V2,
                notification: NotificationMethod::Event {
                    registration_id: registration.id.clone(),
                    target: target.clone(),
                    cursor,
                    event: Box::new(crate::runtime_client::RuntimeClientEvent::TraceChanged),
                },
            },
            Some(registration.event(cursor)),
        ))
        .unwrap();
        socket.send(stale).await.unwrap(); // discarded final handoff, no Frame
        socket.send(r3).await.unwrap();
        assert!(matches!(
            serde_json::from_str::<Response>(
                &peer.next().await.unwrap().unwrap().into_text().unwrap()
            )
            .unwrap(),
            Response::Success(_)
        ));
        assert!(slot.following().is_some());
        connection.close();
        f.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t16_physical_acceptance_wins_before_replacement_no_lock_cycle() {
    use crate::app_server::registration::Acceptance;
    use std::{io::Write, task::Poll};
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &f, 0).await;
        let MethodResult::Checkpoint { checkpoint } = super::protocol::call(
            &connection,
            180,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint")
        };
        super::protocol::call(
            &connection,
            181,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: checkpoint.cursor,
            },
        )
        .await;
        let slot = connection.registration_for_test(&target);
        let (registration, _) = slot.following().unwrap();
        let guard = registration.event(checkpoint.cursor);
        let mut blocked = slot.install_wait_probe();
        // Parallel process fixtures must not inherit this pipe and postpone EOF.
        let (read, write) = std::io::pipe().unwrap();
        let read: std::os::fd::OwnedFd = read.into();
        let write: std::os::fd::OwnedFd = write.into();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let accepting = tokio::task::spawn_blocking(move || {
            let mut write = std::fs::File::from(write);
            guard.hand_off("{}", |record| {
                let written = write.write(record.as_bytes()).unwrap();
                assert!(written > 0); // actual first positive OS pipe write
                entered.send(()).unwrap();
                release_rx.recv().unwrap();
                Poll::Ready(Ok(written))
            })
        });
        entered_rx.await.unwrap();
        let replacing = connection.clone();
        let replacement = tokio::spawn(async move {
            replacing
                .reply(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(182),
                    call: Method::SessionSubscribe {
                        target,
                        after_cursor: checkpoint.cursor,
                    },
                })
                .await
        });
        while !*blocked.borrow_and_update() {
            blocked.changed().await.unwrap();
        }
        // Replacement has reached the real slot and is blocked there; we do
        // not wait for an interleaving the synchronization boundary forbids.
        release.send(()).unwrap();
        assert!(matches!(
            accepting.await.unwrap(),
            Acceptance::Accepted(_, 2)
        ));
        let successor = replacement.await.unwrap().commit();
        assert!(matches!(successor, Response::Success(_)));
        let bytes = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::from(read).read_to_end(&mut bytes).unwrap();
            bytes
        })
        .await
        .unwrap();
        assert_eq!(bytes, b"{}");
        connection.close();
        f.close().await;
    })
    .await;
}

struct FirstByteGate<W> {
    writer: W,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: tokio::sync::oneshot::Receiver<()>,
    accepted: bool,
    released: bool,
}
impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for FirstByteGate<W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        use std::task::{Poll, ready};
        if self.accepted && !self.released {
            ready!(std::pin::Pin::new(&mut self.release).poll(cx)).map_err(io::Error::other)?;
            self.released = true;
        }
        let offered = if self.accepted {
            bytes
        } else {
            &bytes[..bytes.len().min(1)]
        };
        let count = ready!(std::pin::Pin::new(&mut self.writer).poll_write(cx, offered))?;
        if count > 0 && !self.accepted {
            self.accepted = true;
            self.entered.take().unwrap().send(()).unwrap();
        }
        Poll::Ready(Ok(count))
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

/// T17: the actual JSONL writer completes its accepted record after native
/// replacement. A prefix is never complete JSON and cannot be Applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t17_partial_os_pipe_record_finishes_after_registration_replacement() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &f, 0).await;
        let MethodResult::Checkpoint { checkpoint } = super::protocol::call(
            &connection,
            280,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint")
        };
        let r1 = connection
            .reply(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(281),
                call: Method::SessionSubscribe {
                    target: target.clone(),
                    after_cursor: checkpoint.cursor,
                },
            })
            .await;
        let mut outbound = transport::Outbound::reply(r1).unwrap();
        // Parallel process fixtures must not inherit this pipe and postpone EOF.
        let (read, write) = std::io::pipe().unwrap();
        let read: std::os::fd::OwnedFd = read.into();
        let write: std::os::fd::OwnedFd = write.into();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let mut writer = FirstByteGate {
            writer: tokio::net::unix::pipe::Sender::from_owned_fd(write).unwrap(),
            entered: Some(entered),
            release: release_rx,
            accepted: false,
            released: false,
        };
        let writing = tokio::spawn(async move {
            stdio::write_outbound(&mut writer, &mut outbound)
                .await
                .unwrap();
        });
        entered_rx.await.unwrap();
        let mut read = std::fs::File::from(read);
        let (read, prefix) = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut prefix = [0; 1];
            read.read_exact(&mut prefix).unwrap();
            (read, prefix)
        })
        .await
        .unwrap();
        assert!(serde_json::from_slice::<Response>(&prefix).is_err());
        assert!(
            connection
                .registration_for_test(&target)
                .following()
                .is_some()
        );
        super::protocol::call(
            &connection,
            282,
            Method::SessionSubscribe {
                target,
                after_cursor: checkpoint.cursor,
            },
        )
        .await;
        release.send(()).unwrap();
        writing.await.unwrap();
        let remainder = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut read = read;
            let mut remaining = vec![];
            read.read_to_end(&mut remaining).unwrap();
            remaining
        })
        .await
        .unwrap();
        let bytes = [prefix.to_vec(), remainder].concat();
        assert_eq!(bytes.last(), Some(&b'\n'));
        let Response::Success(response) = serde_json::from_slice::<Response>(&bytes).unwrap()
        else {
            panic!("accepted record must not be replaced mid-record")
        };
        assert_eq!(response.id, RequestId::Integer(281));
        connection.close();
        f.close().await;
    })
    .await;
}

/// A sustained saturation deadline retires queued records, but the real OS
/// pipe still owns its accepted prefix and must finish that exact record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t17_t20_stdio_saturation_deadline_finishes_the_physically_accepted_record() {
    use tokio::io::AsyncWriteExt;
    bounded(async {
        eprintln!("T17 saturation deadline: fixture");
        let fixture = Fixture::with_session_count(None, 0).await;
        let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
        super::protocol::initialize(&connection).await;
        eprintln!("T17 saturation deadline: initialized");
        let (reader, mut input) = tokio::io::duplex(65_536);
        // The malformed record produces a bounded native error immediately,
        // before the initial source notification can become the writer head.
        input.write_all(b"{\n").await.unwrap();
        // Parallel process fixtures must not inherit this pipe and postpone EOF.
        let (read, write) = std::io::pipe().unwrap();
        let read: std::os::fd::OwnedFd = read.into();
        let write: std::os::fd::OwnedFd = write.into();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let writer = FirstByteGate {
            writer: tokio::net::unix::pipe::Sender::from_owned_fd(write).unwrap(),
            entered: Some(entered),
            release: release_rx,
            accepted: false,
            released: false,
        };
        let incoming = futures_util::stream::try_unfold(
            tokio::io::BufReader::new(reader),
            |mut reader| async move {
                use tokio::io::AsyncBufReadExt;
                let mut line = String::new();
                Ok((reader.read_line(&mut line).await? != 0)
                    .then(|| (line.trim_end_matches('\n').to_owned(), reader)))
            },
        );
        // Hold the physical writer by a channel, independently of its normal
        // deadline, to prove that the queue deadline still joins this record.
        let (closed, close_watch) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(transport::serve_for_test(
            connection.clone(),
            incoming,
            move |mut outgoing| async move {
                assert!(closed.send(outgoing.retirement_watch()).is_ok());
                let mut writer = writer;
                while let Some(mut outbound) = outgoing.next().await {
                    stdio::write_outbound(&mut writer, &mut outbound).await?;
                }
                Ok(())
            },
            CancellationToken::new(),
        ));
        let mut retirement = close_watch.await.unwrap();
        entered_rx.await.unwrap();
        eprintln!("T17 saturation deadline: positive prefix accepted");
        let reader = std::fs::File::from(read);
        let (reader, prefix) = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut reader = reader;
            let mut prefix = [0; 1];
            reader.read_exact(&mut prefix).unwrap();
            (reader, prefix)
        })
        .await
        .unwrap();
        assert!(serde_json::from_slice::<Response>(&prefix).is_err());
        eprintln!("T17 saturation deadline: incomplete prefix read");
        // All 32 queue slots fill while the real accepted remainder is gated.
        // One further reply waits on the same fixed queue deadline, without
        // any EOF or shutdown. It never adds an encoded buffer.
        input
            .write_all("{\n".repeat(transport::OUTBOUND_MESSAGES + 1).as_bytes())
            .await
            .unwrap();
        eprintln!("T17 saturation deadline: 33 replies fed");
        retirement.retired().await;
        assert!(connection.closed_for_test());
        assert!(
            !serving.is_finished(),
            "overflow must retain the accepted writer"
        );
        eprintln!("T17 saturation deadline: retired before writer completion");
        release.send(()).unwrap();
        let error = serving.await.unwrap().unwrap_err();
        eprintln!("T17 saturation deadline: writer joined: {error}");
        assert_eq!(error.to_string(), "outbound capacity deadline exceeded");
        let remainder = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut reader = reader;
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            bytes
        })
        .await
        .unwrap();
        eprintln!("T17 saturation deadline: physical pipe EOF");
        let bytes = [prefix.to_vec(), remainder].concat();
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert!(!bytes[..bytes.len() - 1].contains(&b'\n'));
        let Response::Failure(response) = serde_json::from_slice::<Response>(&bytes).unwrap()
        else {
            panic!("the exact original parse failure must finish")
        };
        assert_eq!(response.id, None);
        assert_eq!(response.error.code, -32700);
        assert!(fixture.provider.request_bodies().is_empty());
        fixture.close().await;
        eprintln!("T17 saturation deadline: cleanup complete");
    })
    .await;
}

/// A real JSONL carrier retires an offered but unaccepted reply without
/// waiting for the blocked pipe to become writable, on EOF or shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t17_stdio_teardown_retires_unaccepted_record_without_write_readiness() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    bounded(async {
        for eof in [true, false] {
            eprintln!("T17 teardown eof={eof}: fixture");
            let fixture = Fixture::with_session_count(None, 0).await;
            let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
            super::protocol::initialize(&connection).await;
            let valve = ValveControl::new();
            let stop = CancellationToken::new();
            let (mut input, reader) = tokio::io::duplex(65_536);
            let (writer, output) = tokio::io::duplex(65_536);
            let serving = tokio::spawn(stdio::serve(
                connection.clone(),
                reader,
                Valve {
                    inner: writer,
                    control: valve.clone(),
                },
                stop.clone(),
            ));
            // Drive the real carrier with its writer open, then parse the
            // warmup reply and initial source notice before closing the valve.
            // Neither may become an unrelated blocked queue head.
            input
                .write_all(
                    format!("{}\n", serde_json::to_string(&server_info(290)).unwrap()).as_bytes(),
                )
                .await
                .unwrap();
            let mut output = tokio::io::BufReader::new(output);
            let (mut replied, mut noticed) = (false, false);
            while !replied || !noticed {
                let mut initial = String::new();
                assert!(output.read_line(&mut initial).await.unwrap() > 0);
                if response(initial.as_bytes(), 290) == Some(true) {
                    assert!(!replied);
                    replied = true;
                } else {
                    let notice: serde_json::Value = serde_json::from_str(&initial).unwrap();
                    assert_eq!(notice["jsonrpc"], "2.0");
                    assert_eq!(notice["method"], "configuration/sourceRepairChanged");
                    assert_eq!(
                        notice["params"]["target"],
                        serde_json::json!({ "kind": "user" })
                    );
                    assert_eq!(notice["params"]["receipt"], serde_json::Value::Null);
                    assert_eq!(notice["params"]["sequence"], "0");
                    assert!(!noticed);
                    noticed = true;
                }
            }
            // The peer can read the preceding delimiter before poll_write
            // updates its counter. Joining its state lock closes acceptance
            // and includes that completed write in the baseline.
            valve.grant(0);
            let accepted_before = valve.written();
            assert!(accepted_before > 0);
            input
                .write_all(
                    format!("{}\n", serde_json::to_string(&server_info(291)).unwrap()).as_bytes(),
                )
                .await
                .unwrap();
            eprintln!("T17 teardown eof={eof}: request sent; waiting for zero-byte offer");
            let offered = valve.refused(|_| true).await;
            assert_eq!(
                response(&offered, 291),
                Some(true),
                "the first zero-byte offer must be the exact correlated reply: {}",
                String::from_utf8_lossy(&offered[..offered.len().min(4096)])
            );
            eprintln!("T17 teardown eof={eof}: zero-byte offer witnessed");
            assert_eq!(valve.written(), accepted_before, "no reply bytes accepted");
            if eof {
                input.shutdown().await.unwrap();
            } else {
                stop.cancel();
            }
            eprintln!("T17 teardown eof={eof}: carrier retirement signalled; waiting for serve");
            serving.await.unwrap().unwrap();
            eprintln!("T17 teardown eof={eof}: serve complete");
            assert!(connection.closed_for_test());
            let mut bytes = Vec::new();
            output.read_to_end(&mut bytes).await.unwrap();
            eprintln!("T17 teardown eof={eof}: output EOF");
            assert!(
                bytes.is_empty(),
                "teardown cannot accept a new terminal record"
            );
            assert_eq!(valve.written(), accepted_before);
            assert!(fixture.provider.request_bodies().is_empty());
            fixture.close().await;
            eprintln!("T17 teardown eof={eof}: fixture closed");
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t17_stdio_input_eof_finishes_the_physically_accepted_output_record() {
    use tokio::io::AsyncWriteExt;
    bounded(async {
        let fixture = Fixture::with_session_count(None, 1).await;
        let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &fixture, 0).await;
        let MethodResult::Checkpoint { checkpoint } = super::protocol::call(
            &connection,
            290,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint");
        };
        let (input, mut incoming) = tokio::io::duplex(65_536);
        // Parallel process fixtures must not inherit this pipe and postpone EOF.
        let (read, write) = std::io::pipe().unwrap();
        let read: std::os::fd::OwnedFd = read.into();
        let write: std::os::fd::OwnedFd = write.into();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let writer = FirstByteGate {
            writer: tokio::net::unix::pipe::Sender::from_owned_fd(write).unwrap(),
            entered: Some(entered),
            release: release_rx,
            accepted: false,
            released: false,
        };
        let serving = tokio::spawn(stdio::serve(
            connection.clone(),
            input,
            writer,
            CancellationToken::new(),
        ));
        let request = Request {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(291),
            call: Method::SessionSubscribe {
                target,
                after_cursor: checkpoint.cursor,
            },
        };
        let mut record = serde_json::to_vec(&request).unwrap();
        record.push(b'\n');
        incoming.write_all(&record).await.unwrap();
        entered_rx.await.unwrap();
        let reader = std::fs::File::from(read);
        let (reader, prefix) = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut reader = reader;
            let mut prefix = [0; 1];
            reader.read_exact(&mut prefix).unwrap();
            (reader, prefix)
        })
        .await
        .unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&prefix).is_err());
        incoming.shutdown().await.unwrap();
        while !connection.closed_for_test() {
            tokio::task::yield_now().await;
        }
        assert!(
            !serving.is_finished(),
            "input EOF must preserve accepted output ownership"
        );
        release.send(()).unwrap();
        serving.await.unwrap().unwrap();
        let remainder = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut reader = reader;
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            bytes
        })
        .await
        .unwrap();
        let bytes = [prefix.to_vec(), remainder].concat();
        assert_eq!(bytes.last(), Some(&b'\n'));
        let first = bytes.split(|byte| *byte == b'\n').next().unwrap();
        serde_json::from_slice::<serde_json::Value>(first).unwrap();
        fixture.close().await;
    })
    .await;
}

struct PendingWriteProbe<W> {
    writer: W,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
}
impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for PendingWriteProbe<W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let result = std::pin::Pin::new(&mut self.writer).poll_write(cx, bytes);
        if result.is_pending()
            && let Some(entered) = self.entered.take()
        {
            let _ = entered.send(());
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

/// T16: an actual full OS pipe proves Pending accepted zero bytes. Native
/// replacement wakes and discards the stale head without pipe readiness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t16_pending_physical_pipe_replacement_advances_without_readiness() {
    Box::pin(bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &f, 0).await;
        let slot = connection.registration_for_test(&target);
        let (registration, _) = slot.following().unwrap();
        let cursor = crate::runtime_client::types::RuntimeClientCursor::new(0);
        let mut outbound = transport::Outbound::notification((
            Notification {
                jsonrpc: JsonRpcVersion::V2,
                notification: NotificationMethod::Event {
                    registration_id: registration.id.clone(),
                    target: target.clone(),
                    cursor,
                    event: Box::new(crate::runtime_client::RuntimeClientEvent::TraceChanged),
                },
            },
            Some(registration.event(cursor)),
        ))
        .unwrap();
        // Parallel process fixtures must not inherit this pipe and postpone EOF.
        let (read, write) = std::io::pipe().unwrap();
        let read: std::os::fd::OwnedFd = read.into();
        let write: std::os::fd::OwnedFd = write.into();
        let sender = tokio::net::unix::pipe::Sender::from_owned_fd(write).unwrap();
        sender.writable().await.unwrap();
        let fill = [b'x'; 8192];
        let mut filled = 0;
        for bytes in [&fill[..], b"x".as_slice()] {
            loop {
                match nix::unistd::write(&sender, bytes) {
                    Ok(count) => {
                        assert!(count > 0);
                        filled += count;
                    }
                    Err(nix::errno::Errno::EAGAIN) => break,
                    Err(error) => panic!("fill pipe: {error}"),
                }
            }
        }
        assert!(filled > 0);
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let mut writer = PendingWriteProbe {
            writer: sender,
            entered: Some(entered),
        };
        let writing = tokio::spawn(async move {
            stdio::write_outbound(&mut writer, &mut outbound)
                .await
                .unwrap();
        });
        entered_rx.await.unwrap();
        let MethodResult::Checkpoint { checkpoint } = super::protocol::call(
            &connection,
            382,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint");
        };
        super::protocol::call(
            &connection,
            383,
            Method::SessionSubscribe {
                target,
                after_cursor: checkpoint.cursor,
            },
        )
        .await;
        // The reader remains untouched. Completion is possible only because
        // replacement woke the writer and fenced its unaccepted notification.
        writing.await.unwrap();
        let bytes = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::from(read).read_to_end(&mut bytes).unwrap();
            bytes
        })
        .await
        .unwrap();
        assert_eq!(bytes.len(), filled);
        assert!(bytes.iter().all(|byte| *byte == b'x'));
        connection.close();
        f.close().await;
    }))
    .await;
}

/// T17: positive physical acceptance is not client application. Disconnecting
/// with a partial Event replays that exact native cursor from the local cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t17_partial_event_disconnect_replays_from_locally_applied_cursor() {
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = Arc::new(AppServerConnection::new(f.host.clone()));
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &f, 0).await;
        let MethodResult::Checkpoint { checkpoint } = super::protocol::call(
            &connection,
            701,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint");
        };
        super::protocol::call(
            &connection,
            702,
            Method::SessionSubscribe {
                target: target.clone(),
                after_cursor: checkpoint.cursor,
            },
        )
        .await;
        let mut applied = checkpoint.cursor;
        super::protocol::call(
            &connection,
            703,
            Method::TurnStart {
                target: target.clone(),
                content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                    text: "partial event recovery".into(),
                })],
            },
        )
        .await;
        f.gates[0].wait_entered().await;
        let candidate = loop {
            let candidate = connection.next_notification_candidate().await;
            if matches!(&candidate.0.notification, NotificationMethod::Event { .. }) {
                break candidate;
            }
        };
        let NotificationMethod::Event { cursor, event, .. } = &candidate.0.notification else {
            unreachable!()
        };
        let delivered_cursor = *cursor;
        let delivered_event = event.clone();
        assert_eq!(delivered_cursor.get(), applied.get() + 1);
        let mut outbound = transport::Outbound::notification(candidate).unwrap();
        // Parallel process fixtures must not inherit this pipe and postpone EOF.
        let (read, write) = std::io::pipe().unwrap();
        let read: std::os::fd::OwnedFd = read.into();
        let write: std::os::fd::OwnedFd = write.into();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (_release, release_rx) = tokio::sync::oneshot::channel();
        let mut writer = FirstByteGate {
            writer: tokio::net::unix::pipe::Sender::from_owned_fd(write).unwrap(),
            entered: Some(entered),
            release: release_rx,
            accepted: false,
            released: false,
        };
        let writing =
            tokio::spawn(async move { stdio::write_outbound(&mut writer, &mut outbound).await });
        entered_rx.await.unwrap();
        let mut read = std::fs::File::from(read);
        let prefix = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut prefix = [0; 1];
            read.read_exact(&mut prefix).unwrap();
            prefix
        })
        .await
        .unwrap(); // reader closes physically after the one accepted byte
        assert!(serde_json::from_slice::<Notification>(&prefix).is_err());
        assert_eq!(
            connection.registration_for_test(&target).accepted_cursor(),
            Some(delivered_cursor)
        );
        assert_eq!(applied, checkpoint.cursor);
        writing.abort();
        assert!(writing.await.unwrap_err().is_cancelled()); // no residual pipe write survives
        connection.close();
        let runtime = f.manager.load(&target.session_id, None).await.unwrap();
        assert!(runtime.inspect_runtime().unwrap().has_current_attempt());
        assert_eq!(runtime.incarnation_id(), target.runtime_incarnation);
        let replacement = AppServerConnection::new(f.host.clone());
        super::protocol::initialize(&replacement).await;
        let MethodResult::Attached {
            target: successor, ..
        } = super::protocol::call(
            &replacement,
            704,
            Method::SessionAttach {
                session_id: target.session_id,
                node_id: None,
            },
        )
        .await
        else {
            panic!("identity receipt");
        };
        assert_eq!(successor.runtime_incarnation, target.runtime_incarnation);
        super::protocol::call(
            &replacement,
            705,
            Method::SessionSubscribe {
                target: successor,
                after_cursor: applied,
            },
        )
        .await;
        let recovered = loop {
            let notification = replacement.next_notification().await;
            if matches!(&notification.notification, NotificationMethod::Event { .. }) {
                break notification;
            }
        };
        let record = serde_json::to_string(&recovered).unwrap();
        let parsed: Notification = serde_json::from_str(&record).unwrap();
        let NotificationMethod::Event { cursor, event, .. } = parsed.notification else {
            unreachable!()
        };
        assert_eq!(cursor.get(), applied.get() + 1);
        assert_eq!(cursor, delivered_cursor);
        assert_eq!(event, delivered_event);
        let crate::runtime_client::RuntimeClientEvent::ProjectionTransition { metadata, .. } =
            &*event
        else {
            panic!("complete native fold transition");
        };
        assert_eq!(metadata.read_cut.revision, cursor.get());
        applied = cursor; // only the complete parsed contiguous native transition advances it
        assert_eq!(applied, delivered_cursor);
        f.gates[0].release();
        replacement.close();
        f.close().await;
    })
    .await;
}

/// T16/T17: tungstenite owns the Frame even when its immediate stream write
/// fails. That acceptance commits once; it is not evidence of client application.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t16_t17_websocket_frame_handoff_precedes_physical_write_failure() {
    use tokio::io::AsyncWriteExt;
    use tokio_tungstenite::tungstenite::protocol::Role;
    bounded(async {
        let f = Fixture::with_session_count(None, 1).await;
        let connection = AppServerConnection::new(f.host.clone());
        super::protocol::initialize(&connection).await;
        let target = super::protocol::attach(&connection, &f, 0).await;
        let MethodResult::Checkpoint { checkpoint } = super::protocol::call(
            &connection,
            710,
            Method::SessionCheckpoint {
                target: target.clone(),
                trace_records: vec![],
            },
        )
        .await
        else {
            panic!("checkpoint");
        };
        let receipt = transport::Outbound::reply(
            connection
                .reply(Request {
                    jsonrpc: JsonRpcVersion::V2,
                    id: RequestId::Integer(711),
                    call: Method::SessionSubscribe {
                        target: target.clone(),
                        after_cursor: checkpoint.cursor,
                    },
                })
                .await,
        )
        .unwrap();
        assert!(
            connection
                .registration_for_test(&target)
                .following()
                .is_none()
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, server) = tokio::join!(
            tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept()
        );
        let mut stream = server.unwrap().0;
        stream.shutdown().await.unwrap(); // subsequent actual kernel write is refused
        let socket = websocket::Socket::new(
            tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Server, None).await,
        );
        assert!(socket.send(receipt).await.is_err());
        assert!(
            connection
                .registration_for_test(&target)
                .following()
                .is_some(),
            "the Frame handoff precedes its I/O failure"
        );
        let mut client = client.unwrap();
        let mut bytes = vec![];
        tokio::io::AsyncReadExt::read_to_end(&mut client, &mut bytes)
            .await
            .unwrap();
        assert!(
            bytes.is_empty(),
            "the client cannot apply an unreceived Frame"
        );
        drop(socket);
        connection.close();
        f.close().await;
    })
    .await;
}

async fn read_effective_configuration_bytes(
    client: &impl AppServerConformanceDriver,
    target: &AttachmentTarget,
    reference: &crate::local_runtime::configuration::effective_observation::EffectiveFieldRef,
) -> Vec<u8> {
    use base64::Engine as _;
    let mut bytes = Vec::new();
    let mut offset = 0;
    loop {
        let MethodResult::ConfigurationValueChunk { chunk } = call(
            client,
            102,
            Method::ConfigurationValueRead {
                target: target.clone(),
                reference: reference.clone(),
                offset,
                max_bytes: 65_536,
            },
        )
        .await
        else {
            panic!("correlated native field range");
        };
        assert_eq!(&chunk.reference, reference);
        assert_eq!(chunk.offset, offset);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&chunk.data_base64)
            .unwrap();
        assert!(!decoded.is_empty());
        assert!(decoded.len() <= 65_536);
        bytes.extend(decoded);
        let Some(next) = chunk.next_offset else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(u64::try_from(bytes.len()).unwrap(), reference.byte_length);
    bytes
}

async fn await_effective_read_execution(
    client: &impl AppServerConformanceDriver,
    target: &AttachmentTarget,
) {
    loop {
        if let NotificationMethod::Event {
            target: delivered,
            event,
            ..
        } = client.next_notification().await.notification
            && &delivered == target
            && event.facts().iter().any(|fact| {
                matches!(
                    fact,
                    crate::runtime_client::RuntimeClientEvent::AttemptSettled { .. }
                )
            })
        {
            break;
        }
    }
}

async fn start_effective_read_secondary(
    client: &impl AppServerConformanceDriver,
    f: &Fixture,
) -> AttachmentTarget {
    let MethodResult::Attached { target, .. } = call(
        client,
        106,
        Method::SessionAttach {
            session_id: f.sessions[1].id.clone(),
            node_id: None,
        },
    )
    .await
    else {
        panic!("secondary identity");
    };
    let MethodResult::Checkpoint { checkpoint } = call(
        client,
        107,
        Method::SessionCheckpoint {
            target: target.clone(),
            trace_records: vec![],
        },
    )
    .await
    else {
        panic!("secondary native checkpoint");
    };
    call(
        client,
        108,
        Method::SessionSubscribe {
            target: target.clone(),
            after_cursor: checkpoint.cursor,
        },
    )
    .await;
    call(
        client,
        109,
        Method::TurnStart {
            target: target.clone(),
            content: vec![UserInputBlock::Text(crate::message::content::TextBlock {
                text: "request-B: unrelated execution across configuration reads".into(),
            })],
        },
    )
    .await;
    f.gates[1].wait_entered().await;
    target
}

/// Every target-scoped read must reach its runtime route, including the
/// continuation methods whose public getter has already returned a descriptor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_t15_effective_configuration_ranges_survive_model_commit_on_real_websocket() {
    use crate::local_runtime::configuration::effective_observation::EffectiveFieldName;
    use crate::model::{catalog::ModelRef, session::SessionModelConfig};
    bounded(Box::pin(async {
        let f = Fixture::new().await;
        let (client, serving) = connected(&f, true).await;
        initialize(&client).await;
        let target = attach(&client, &f).await;
        let secondary = start_effective_read_secondary(&client, &f).await;
        let mut config = SessionModelConfig::of(ModelRef::parse("local/a").unwrap());
        config
            .request_params
            .insert("range".into(), serde_json::json!("é\\\"".repeat(40_000)));
        assert!(matches!(
            call(
                &client,
                100,
                Method::ModelSet {
                    target: target.clone(),
                    config: Box::new(config),
                }
            )
            .await,
            MethodResult::ModelSet { .. }
        ));
        let MethodResult::EffectiveConfiguration { projection } = call(
            &client,
            101,
            Method::ConfigurationGet {
                target: target.clone(),
            },
        )
        .await
        else {
            panic!("bounded configuration");
        };
        let reference = projection
            .fields
            .iter()
            .filter_map(|field| field.reference.as_ref())
            .find(|reference| reference.field == EffectiveFieldName::EffectiveModel)
            .unwrap()
            .clone();
        assert!(reference.byte_length > 65_536);
        let bytes = read_effective_configuration_bytes(&client, &target, &reference).await;
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            original["configured"]["requestParams"]["range"],
            "é\\\"".repeat(40_000)
        );
        let MethodResult::ConfigurationPage { page } = call(
            &client,
            103,
            Method::ConfigurationPage {
                target: target.clone(),
                revision: projection.revision.clone(),
                field: EffectiveFieldName::Provenance,
                offset: 0,
                limit: 32,
            },
        )
        .await
        else {
            panic!("correlated configuration continuation");
        };
        assert_eq!(page.revision, projection.revision);
        assert!(page.items.len() <= 32);
        assert!(matches!(
            call(&client, 104, Method::ServerInfo {}).await,
            MethodResult::ServerInfo { .. }
        ));
        f.gates[1].release();
        await_effective_read_execution(&client, &secondary).await;
        assert!(!serving.is_finished(), "field reads keep the carrier alive");
        client.close().await;
        // Driver::close deliberately aborts its socket pumps; the resulting
        // reset is fixture teardown, after every correlated progress assertion.
        let _ = serving.await.unwrap();
        f.close().await;
    }))
    .await;
}
