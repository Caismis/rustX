//! Retired carriers release `ConnectionLease` while native waits retain the one Host counter.
use super::*;
use crate::app_server::transport::websocket;
use crate::local_runtime::session_runtime_manager::Entry;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const TOKEN: &str = "test-token-000000000000000000000000000000000000000";

async fn send(socket: &mut Socket, id: i64, call: Method) {
    let record = serde_json::to_string(&Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(id),
        call,
    })
    .unwrap();
    socket.send(record.into()).await.unwrap();
}

async fn next_response(socket: &mut Socket) -> Response {
    loop {
        let frame = socket.next().await.unwrap().unwrap();
        let tokio_tungstenite::tungstenite::Message::Text(record) = frame else {
            continue;
        };
        assert!(record.len() <= crate::app_server::transport::MAX_MESSAGE_BYTES);
        if let Ok(response) = serde_json::from_str::<Response>(&record) {
            return response;
        }
        serde_json::from_str::<Notification>(&record).expect("bounded native notification");
    }
}

async fn response(socket: &mut Socket, id: i64) -> Response {
    let response = next_response(socket).await;
    let received = match &response {
        Response::Success(value) => Some(&value.id),
        Response::Failure(value) => value.id.as_ref(),
    };
    assert_eq!(received, Some(&RequestId::Integer(id)));
    response
}

async fn connect(url: &str, child: &ChildDelivery) -> (Socket, AttachmentTarget) {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{}, rustx-token.{TOKEN}", websocket::SUBPROTOCOL)
            .parse()
            .unwrap(),
    );
    eprintln!("global reconnect: TCP handshake start");
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    eprintln!("global reconnect: TCP handshake complete");
    send(
        &mut socket,
        0,
        Method::Initialize(InitializeParams {
            protocol_version: APP_SERVER_PROTOCOL_VERSION,
            client: ClientIdentity {
                name: "control-reconnect".into(),
                version: "1".into(),
            },
            presentation: PresentationCapabilities::default(),
        }),
    )
    .await;
    assert!(matches!(
        response(&mut socket, 0).await,
        Response::Success(_)
    ));
    eprintln!("global reconnect: initialized; attach start");
    send(
        &mut socket,
        1,
        Method::SessionAttach {
            session_id: child.target.session_id.clone(),
            node_id: None,
        },
    )
    .await;
    let Response::Success(success) = response(&mut socket, 1).await else {
        panic!("native reattachment")
    };
    let MethodResult::Attached { target, .. } = success.result else {
        panic!("attachment identity")
    };
    eprintln!("global reconnect: actual attachment receipt");
    assert_eq!(target.conversation_id, child.target.conversation_id);
    assert_eq!(target.runtime_incarnation, child.target.runtime_incarnation);
    (socket, target)
}

async fn waits(
    socket: &mut Socket,
    child: &ChildDelivery,
    target: &AttachmentTarget,
    job: &crate::runtime::identity::ToolExecutionId,
    count: i64,
) {
    for index in 0..count {
        send(
            socket,
            100 + index,
            if index % 2 == 0 {
                Method::JobWait {
                    target: target.clone(),
                    job_id: job.clone(),
                }
            } else {
                Method::AgentWait {
                    target: target.clone(),
                    agent_id: child.agent.clone(),
                }
            },
        )
        .await;
    }
}

async fn count(receiver: &mut tokio::sync::watch::Receiver<usize>, expected: usize) {
    eprintln!(
        "global reconnect: native watch barrier expected={expected} current={}",
        *receiver.borrow()
    );
    receiver.wait_for(|count| *count == expected).await.unwrap();
    eprintln!("global reconnect: native watch barrier reached={expected}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t14_t20_reaped_websockets_preserve_global_control_reserves_over_native_waits() {
    bounded(Box::pin(async {
        eprintln!("global reconnect: real Child setup start");
        let child = ChildDelivery::with_delivery(false).await;
        eprintln!("global reconnect: real Child setup complete");
        child.connection.close();
        let host = child.fixture.host.clone();
        let mut policy = host.policy(); policy.max_connections = 1;
        child.fixture.manager.bind_process_policy(policy);
        let runtime = child.fixture.manager.configuration_runtime(&child.target.session_id).unwrap();
        let registry = runtime.subagent_registry().unwrap();
        let activation = registry.agent_snapshot(&child.agent).unwrap().current_activation.unwrap();
        let terminal = Arc::new(crate::runtime::subagent::TerminalAuthorityHook::default());
        registry.install_terminal_authority_hook(terminal.clone());
        let job_gate = Arc::new(super::super::super::super::AsyncGate::default()); job_gate.arm();
        let release = Release { job: job_gate.clone(), child: terminal.clone() };
        let (job, cancellation) = held_job(&runtime, job_gate.clone()).await;
        eprintln!("global reconnect: original background executor held");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let stop = tokio_util::sync::CancellationToken::new();
        let (slots, mut reaped) = tokio::sync::watch::channel(0);
        let serving = tokio::spawn(websocket::serve_listener(listener, host.clone(), websocket::Credential::new(TOKEN.into()).unwrap(), stop.clone(), Some(slots)));
        let mut operations = host.request_operations();
        let mut admissions = {
            let registry = child.fixture.manager.registry.0.lock().unwrap();
            let Entry::Loaded(resident) = &registry.entries[&child.target.conversation_id] else { panic!("resident") };
            resident.operations.subscribe()
        };
        let mut previous = None;
        for total in (6..=42).step_by(6) {
            let (mut socket, target) = connect(&url, &child).await;
            assert_ne!(previous.as_ref(), Some(&target.attachment_id));
            previous = Some(target.attachment_id.clone());
            count(&mut reaped, 1).await;
            waits(&mut socket, &child, &target, &job, 6).await;
            eprintln!("global reconnect: six native waits written total={total}");
            count(&mut operations, total).await;
            drop(socket); // Real TCP EOF retires the carrier and attachment, not the waits.
            count(&mut reaped, 0).await;
            assert_eq!(host.diagnostics().transport.websocket_connections, 0);
            assert_eq!(*operations.borrow(), total);
            count(&mut admissions, 0).await;
            assert_eq!(host.diagnostics().sessions.iter().find(|row| row.conversation_id == target.conversation_id).unwrap().operations, 0);
        }
        let (mut socket, target) = connect(&url, &child).await;
        assert_ne!(previous.as_ref(), Some(&target.attachment_id));
        count(&mut reaped, 1).await;
        waits(&mut socket, &child, &target, &job, 6).await;
        eprintln!("global reconnect: six fresh native waits written");
        count(&mut operations, 48).await;
        send(&mut socket, 200, Method::ServerInfo {}).await;
        assert!(matches!(response(&mut socket, 200).await, Response::Failure(failure) if failure.error.data == Some(ErrorData::RequestCapacity)));
        eprintln!("global reconnect: ordinary capacity refused; native intents start");
        send(&mut socket, 201, Method::JobCancel { target: target.clone(), job_id: job.clone() }).await;
        send(&mut socket, 202, Method::AgentInterrupt { target: target.clone(), agent_id: child.agent.clone() }).await;
        // Both complete native intent records must physically reach the fresh carrier.
        let mut receipts = std::collections::BTreeSet::new();
        for _ in 0..2 {
            let Response::Success(value) = next_response(&mut socket).await else { panic!("reserved control receipt") };
            match value.result {
                MethodResult::JobCancellationAccepted { job_id } => { assert_eq!(job_id, job); assert_eq!(value.id, RequestId::Integer(201)); receipts.insert(201); }
                MethodResult::AgentInterruptionAccepted { agent_id, activation_id } => { assert_eq!(agent_id, child.agent); assert_eq!(activation_id, Some(activation.clone())); assert_eq!(value.id, RequestId::Integer(202)); receipts.insert(202); }
                value => panic!("native intent: {value:?}"),
            }
        }
        assert_eq!(receipts, [201, 202].into_iter().collect());
        eprintln!("global reconnect: both native intent receipts physically received");
        count(&mut operations, 48).await;
        assert!(cancellation.lock().unwrap().as_ref().unwrap().is_cancelled());
        assert_eq!(job_gate.parked_now(), 1);
        assert_eq!(runtime.tool_runtime().background().snapshot(&job).unwrap().state, crate::tools::background::BackgroundLifecycle::Cancelling);
        let hook = terminal.clone(); tokio::task::spawn_blocking(move || hook.wait_until_entered()).await.unwrap();
        assert!(!registry.snapshot(&activation).unwrap().is_settled());
        drop(socket); count(&mut reaped, 0).await;
        assert_eq!(*operations.borrow(), 48);
        release.job.release(); release.child.release();
        count(&mut operations, 0).await;
        assert_eq!(runtime.tool_runtime().background().snapshot(&job).unwrap().state, crate::tools::background::BackgroundLifecycle::Succeeded);
        assert!(registry.snapshot(&activation).unwrap().is_settled());
        stop.cancel(); serving.await.unwrap().unwrap();
        drop(release); child.close().await;
    })).await;
}
