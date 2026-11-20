//! UI transport detach while the real Client's data and control lanes are busy.
use super::*;
use crate::app_server::transport::websocket;
use futures_util::{SinkExt as _, StreamExt as _};
use rustx_execution_protocol::process::{
    OperationState, ProcessOperationKey, ProcessReply, ProcessRequest, SettledDisposition,
    SettledSnapshot,
};
use rustx_execution_protocol::transport::{
    MAX_CONTROL_COMMANDS, MAX_RECORD_BYTES, read_record, write_record,
};
use rustx_execution_protocol::{
    Bootstrap, BootstrapEndpoint, ClientReply, ClientRequest, ControlPoll, ControlReply,
    ControlTurn, DecimalU64, ProcessCancelDisposition,
};
use std::io;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

async fn receive<T: serde::de::DeserializeOwned>(socket: &mut UnixStream) -> io::Result<T> {
    let length = socket.read_u32().await? as usize;
    assert!((1..=MAX_RECORD_BYTES).contains(&length));
    let mut bytes = vec![0; length];
    socket.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

async fn send(socket: &mut UnixStream, value: &impl serde::Serialize) -> io::Result<()> {
    let mut bytes = Vec::new();
    write_record(&mut bytes, value)?;
    socket.write_all(&bytes).await
}

async fn forward_hello(client: &mut UnixStream, server: &mut UnixStream) {
    let hello = receive::<rustx_execution_protocol::PeerHello>(client)
        .await
        .unwrap();
    send(server, &hello).await.unwrap();
    let reply = receive::<serde_json::Value>(server).await.unwrap();
    send(client, &reply).await.unwrap();
}

#[derive(Debug, Default)]
struct ProcessTrace {
    registered: Vec<ProcessOperationKey>,
    started: Vec<ProcessOperationKey>,
    settlement: Option<SettledSnapshot>,
}

struct Released {
    key: ProcessOperationKey,
    final_sequence: DecimalU64,
    trace: ProcessTrace,
}

async fn proxy_data(
    mut client: UnixStream,
    mut server: UnixStream,
    parked: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
    released: mpsc::UnboundedSender<Released>,
) -> io::Result<()> {
    forward_hello(&mut client, &mut server).await;
    let mut gate = Some((parked, resume));
    let mut trace = ProcessTrace::default();
    while let Ok(request) = receive::<ClientRequest>(&mut server).await {
        send(&mut client, &request).await?;
        let reply = receive::<ClientReply>(&mut client).await?;
        if matches!(request, ClientRequest::GetMetadata { .. })
            && let Some((parked, resume)) = gate.take()
        {
            assert!(matches!(reply, ClientReply::Metadata { .. }));
            parked.send(()).unwrap();
            resume.await.unwrap();
        }
        match (&request, &reply) {
            (ClientRequest::Process(ProcessRequest::Register { key, .. }), _) => {
                trace.registered.push(key.clone());
            }
            (ClientRequest::Process(ProcessRequest::Start { key }), _) => {
                trace.started.push(key.clone());
            }
            (
                _,
                ClientReply::Process(ProcessReply::State {
                    state: OperationState::Settled(snapshot),
                    ..
                }),
            ) => {
                trace.settlement = Some(*snapshot);
            }
            _ => {}
        }
        send(&mut server, &reply).await?;
        if let ClientRequest::Process(ProcessRequest::ReleaseOutput {
            key,
            final_sequence,
        }) = request
        {
            assert_eq!(
                reply,
                ClientReply::Process(ProcessReply::OutputReleased { key: key.clone() })
            );
            released
                .send(Released {
                    key,
                    final_sequence,
                    trace: std::mem::take(&mut trace),
                })
                .unwrap();
        }
    }
    Ok(())
}

async fn proxy_control(
    mut client: UnixStream,
    mut server: UnixStream,
    parked: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
    cancels: mpsc::UnboundedSender<(ProcessOperationKey, ProcessCancelDisposition)>,
) -> io::Result<()> {
    forward_hello(&mut client, &mut server).await;
    let mut gate = Some((parked, resume));
    while let Ok(poll) = receive::<ControlPoll>(&mut client).await {
        if matches!(poll.receipt, Some(ControlReply::Healthy { .. }))
            && let Some((parked, resume)) = gate.take()
        {
            parked.send(()).unwrap();
            resume.await.unwrap();
        }
        send(&mut server, &poll).await?;
        if let Some(ControlReply::ProcessCancelled { key, disposition }) = poll.receipt {
            cancels.send((key, disposition)).unwrap();
        }
        let turn = receive::<ControlTurn>(&mut server).await?;
        send(&mut client, &turn).await?;
    }
    Ok(())
}

async fn connect_ui(
    url: String,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        format!(
            "{}, rustx-token.{}",
            websocket::SUBPROTOCOL,
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(TOKEN)
        )
        .parse()
        .unwrap(),
    );
    let (mut ui, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ui.send(ui_call(
        0,
        Method::Initialize(InitializeParams {
            protocol_version: APP_SERVER_PROTOCOL_VERSION,
            client: ClientIdentity {
                name: "detach/exec".into(),
                version: "1".into(),
            },
            presentation: PresentationCapabilities::default(),
        }),
    ))
    .await
    .unwrap();
    assert!(matches!(
        serde_json::from_str::<Response>(ui.next().await.unwrap().unwrap().to_text().unwrap())
            .unwrap(),
        Response::Success(_)
    ));
    ui
}

const TOKEN: &str = "command-detach-integration-test-token-000000000000";

fn ui_call(id: i64, call: Method) -> tokio_tungstenite::tungstenite::Message {
    serde_json::to_string(&Request {
        jsonrpc: JsonRpcVersion::V2,
        id: RequestId::Integer(id),
        call,
    })
    .unwrap()
    .into()
}

#[allow(clippy::too_many_lines)] // Keep the barrier ordering together in the scenario.
pub(crate) async fn detach_during_data_and_control_backpressure() {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let endpoint = fixture
        .manager
        .reserve_local_execution(&fixture.sessions[0].id)
        .await
        .unwrap();
    let scratch = tempfile::tempdir().unwrap();
    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let proxy_path = scratch.path().canonicalize().unwrap().join("proxy.sock");
    let proxy = UnixListener::bind(&proxy_path).unwrap();
    let mut bootstrap: Bootstrap =
        read_record(&mut std::fs::File::open(endpoint.bootstrap_path()).unwrap()).unwrap();
    let BootstrapEndpoint::Unix { socket_path } = bootstrap.endpoint else {
        panic!("local Session")
    };
    bootstrap.endpoint = BootstrapEndpoint::Unix {
        socket_path: proxy_path,
    };
    let bundle = scratch.path().join("binding");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&bundle)
        .unwrap();
    write_record(&mut file, &bootstrap).unwrap();
    drop(file);
    let client = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    let mut child = tokio::process::Command::new(client)
        .arg("--binding-file")
        .arg(bundle.canonicalize().unwrap())
        .arg("--workspace")
        .arg(endpoint.workspace())
        .env_clear()
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let rustx::local_runtime::session_runtime_manager::ExecutionListener::Unix(listener) =
        endpoint.listener()
    else {
        panic!("Unix endpoint")
    };
    listener.set_nonblocking(true).unwrap();
    let listener = UnixListener::from_std(listener.try_clone().unwrap()).unwrap();
    let (data_waiting, data_parked) = oneshot::channel();
    let (resume_data, data_resume) = oneshot::channel();
    let (output_release, mut released) = mpsc::unbounded_channel();
    let (peer, _) = bounded(proxy.accept()).await.unwrap();
    let server = UnixStream::connect(&socket_path).await.unwrap();
    let data = tokio::spawn(proxy_data(
        peer,
        server,
        data_waiting,
        data_resume,
        output_release,
    ));
    let (stream, _) = bounded(listener.accept()).await.unwrap();
    {
        let endpoint = endpoint.clone();
        let accepted = tokio::task::spawn_blocking(move || {
            endpoint.admit_local_stream(stream.into_std().unwrap())
        })
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            accepted,
            rustx::local_runtime::session_runtime_manager::EndpointAdmission::Pending
        ));
    }

    let (control_waiting, control_parked) = oneshot::channel();
    let (resume_control, control_resume) = oneshot::channel();
    let (cancel_receipt, mut cancels) = mpsc::unbounded_channel();
    let (peer, _) = bounded(proxy.accept()).await.unwrap();
    let server = UnixStream::connect(&socket_path).await.unwrap();
    let control_task = tokio::spawn(proxy_control(
        peer,
        server,
        control_waiting,
        control_resume,
        cancel_receipt,
    ));
    let (stream, _) = bounded(listener.accept()).await.unwrap();
    {
        let endpoint = endpoint.clone();
        let accepted = tokio::task::spawn_blocking(move || {
            endpoint.admit_local_stream(stream.into_std().unwrap())
        })
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(
            accepted,
            rustx::local_runtime::session_runtime_manager::EndpointAdmission::Live
        ));
    }

    let ui_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", ui_listener.local_addr().unwrap());
    let stop = tokio_util::sync::CancellationToken::new();
    let (slots, mut reaped) = tokio::sync::watch::channel(0);
    let serving = tokio::spawn(websocket::serve_listener(
        ui_listener,
        fixture.host.clone(),
        websocket::Credential::new(TOKEN.into()).unwrap(),
        stop.clone(),
        Some(slots),
    ));
    let mut ui = bounded(connect_ui(url)).await;
    bounded(reaped.wait_for(|count| *count == 1)).await.unwrap();

    // The business process holds a TCP descriptor until physically killed.
    // This observes termination even while the Client cannot answer Query.
    let business_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = business_listener.local_addr().unwrap().port();
    let script = format!(
        "exec 3<>/dev/tcp/127.0.0.1/{port}; printf '%s\\n' \"$$\" >&3; printf READY; read -r -u 3"
    );
    let mut params = parameters(&fixture, &["/bin/bash", "-c", &script]);
    params.process_id = Some("detach-both-lanes".into());
    params.stream_stdout_stderr = true;
    ui.send(ui_call(1, Method::CommandExec(params)))
        .await
        .unwrap();
    let (business, _) = bounded(business_listener.accept()).await.unwrap();
    let mut business = tokio::io::BufReader::new(business);
    let mut pid = String::new();
    bounded(business.read_line(&mut pid)).await.unwrap();
    let pid = pid.trim().parse::<i32>().unwrap();
    let mut shown = Vec::new();
    while shown.len() < 5 {
        let message = bounded(ui.next()).await.unwrap().unwrap();
        let record = message.to_text().unwrap();
        assert!(
            serde_json::from_str::<Response>(record).is_err(),
            "unexpected response before detach"
        );
        if let NotificationMethod::CommandOutputDelta(delta) =
            serde_json::from_str::<Notification>(record)
                .unwrap()
                .notification
        {
            assert_eq!(delta.process_id, "detach-both-lanes");
            shown.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(delta.delta_base64)
                    .unwrap(),
            );
        }
    }
    assert_eq!(shown, b"READY");

    let query = {
        let endpoint = endpoint.clone();
        tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                match endpoint.entry_metadata(endpoint.workspace()) {
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::yield_now();
                    }
                    other => return other,
                }
            }
        })
    };
    bounded(data_parked).await.unwrap();
    let (health_results, mut health) = mpsc::unbounded_channel();
    let handle = endpoint.control().unwrap();
    for _ in 0..=MAX_CONTROL_COMMANDS {
        let handle = handle.clone();
        let health_results = health_results.clone();
        tokio::task::spawn_blocking(move || health_results.send(handle.health()).unwrap());
    }
    drop(health_results);
    bounded(control_parked).await.unwrap();
    assert_eq!(
        bounded(health.recv()).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(matches!(
        health.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    drop(ui);
    bounded(reaped.wait_for(|count| *count == 0)).await.unwrap();
    resume_control.send(()).unwrap();
    let (key, disposition) = bounded(cancels.recv()).await.unwrap();
    assert_eq!(disposition, ProcessCancelDisposition::CancellationRequested);
    for _ in 0..MAX_CONTROL_COMMANDS {
        bounded(health.recv()).await.unwrap().unwrap();
    }
    assert!(bounded(health.recv()).await.is_none());
    assert_eq!(bounded(business.read(&mut [0])).await.unwrap(), 0);
    assert!(!query.is_finished());
    assert!(matches!(
        released.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    resume_data.send(()).unwrap();
    assert!(bounded(query).await.unwrap().unwrap().is_directory);
    let release = bounded(released.recv()).await.unwrap();
    assert_eq!(release.key, key);
    assert_eq!(release.trace.registered, std::slice::from_ref(&key));
    assert_eq!(release.trace.started, [key]);
    let settlement = release.trace.settlement.unwrap();
    assert_eq!(settlement.disposition, SettledDisposition::Cancelled);
    assert_eq!(settlement.final_sequence, release.final_sequence);
    assert_eq!(settlement.stdout_total.get(), 5);
    assert_eq!(settlement.stderr_total.get(), 0);
    process_gone(pid);

    stop.cancel();
    bounded(serving).await.unwrap().unwrap();
    assert_eq!(bounded(fixture.host.drain()).await, [] as [String; 0]);
    assert!(bounded(child.wait()).await.unwrap().success());
    bounded(data).await.unwrap().unwrap();
    if let Err(error) = bounded(control_task).await.unwrap() {
        assert!(
            matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::UnexpectedEof
            ),
            "control proxy could not close after Client cleanup: {error}"
        );
    }
    assert!(released.try_recv().is_err(), "one physical release");
    assert!(cancels.try_recv().is_err(), "one physical cancellation");
}
