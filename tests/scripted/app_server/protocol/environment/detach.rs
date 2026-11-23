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

struct ProxiedClient {
    data: (UnixStream, UnixStream),
    control: (UnixStream, UnixStream),
    endpoint: Arc<rustx::local_runtime::session_runtime_manager::LocalExecutionEndpoint>,
    child: tokio::process::Child,
    _scratch: tempfile::TempDir,
}

async fn proxied_client(fixture: &Fixture) -> ProxiedClient {
    use rustx::local_runtime::session_runtime_manager::EndpointAdmission;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
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
    let child = tokio::process::Command::new(client)
        .arg("--binding-file")
        .arg(bundle.canonicalize().unwrap())
        .arg("--workspace")
        .arg(endpoint.workspace())
        .env_clear()
        .env("TMPDIR", scratch.path())
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
    let pair_lane = || async {
        let (mut peer, _) = bounded(proxy.accept()).await.unwrap();
        let mut server = UnixStream::connect(&socket_path).await.unwrap();
        let (stream, _) = bounded(listener.accept()).await.unwrap();
        let admit = tokio::task::spawn_blocking({
            let endpoint = endpoint.clone();
            move || endpoint.admit_local_stream(stream.into_std().unwrap())
        });
        forward_hello(&mut peer, &mut server).await;
        ((peer, server), admit.await.unwrap().unwrap())
    };
    let (data, admission) = pair_lane().await;
    assert!(matches!(admission, EndpointAdmission::Pending));
    let (control, admission) = pair_lane().await;
    assert!(matches!(admission, EndpointAdmission::Live));
    ProxiedClient {
        data,
        control,
        endpoint,
        child,
        _scratch: scratch,
    }
}

#[allow(clippy::too_many_lines)] // Keep the barrier ordering together in the scenario.
pub(crate) async fn detach_during_data_and_control_backpressure() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let ProxiedClient {
        data: data_lane,
        control,
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let (data_waiting, data_parked) = oneshot::channel();
    let (resume_data, data_resume) = oneshot::channel();
    let (output_release, mut released) = mpsc::unbounded_channel();
    let data = tokio::spawn(proxy_data(
        data_lane.0,
        data_lane.1,
        data_waiting,
        data_resume,
        output_release,
    ));
    let (control_waiting, control_parked) = oneshot::channel();
    let (resume_control, control_resume) = oneshot::channel();
    let (cancel_receipt, mut cancels) = mpsc::unbounded_channel();
    let control_task = tokio::spawn(proxy_control(
        control.0,
        control.1,
        control_waiting,
        control_resume,
        cancel_receipt,
    ));

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

pub(crate) async fn lost_control_is_unconfirmed_in_host_drain() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let ProxiedClient {
        data: (mut data_peer, mut data_server),
        control: (mut control_peer, mut control_server),
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let (stop, stopped) = oneshot::channel::<()>();
    let control = tokio::spawn(async move {
        tokio::select! {
            _ = stopped => {},
            _ = tokio::io::copy_bidirectional(&mut control_peer, &mut control_server) => {
                panic!("both control peers stay connected until the test cuts the lane")
            }
        }
    });
    let (eof, closed) = oneshot::channel();
    let data = tokio::spawn(async move {
        // No client data messages are expected until the Host cuts the pair.
        let length = data_server.read(&mut [0]).await.unwrap();
        assert_eq!(length, 0);
        eof.send(()).unwrap();
        let _ = data_peer.shutdown().await;
    });
    let live = endpoint.control().unwrap();
    bounded(tokio::task::spawn_blocking(move || live.health()))
        .await
        .unwrap()
        .unwrap();
    stop.send(()).unwrap();
    bounded(control).await.unwrap();
    bounded(closed).await.unwrap();
    assert!(endpoint.control().is_none());
    assert_eq!(fixture.host.file_observers().available_permits(), 32);
    let failures = bounded(fixture.host.drain()).await;
    assert_eq!(
        failures,
        [
            "Client revocation is unconfirmed: admitted Client has no control lane or physical settlement receipt"
        ]
    );
    bounded(data).await.unwrap();
    let _client_exit = bounded(child.wait()).await.unwrap();
}

pub(crate) async fn refused_register_does_not_allocate_a_cancellation_tombstone() {
    use rustx_execution_protocol::process::SpecRefusal;
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let ProxiedClient {
        data: (mut data_peer, mut data_server),
        control: (mut control_peer, mut control_server),
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let (refused, mut refusals) = mpsc::unbounded_channel();
    let data = tokio::spawn(async move {
        while let Ok(request) = receive::<ClientRequest>(&mut data_server).await {
            send(&mut data_peer, &request).await.unwrap();
            let reply = receive::<ClientReply>(&mut data_peer).await.unwrap();
            if let ClientReply::Process(ProcessReply::Refused {
                key,
                reason: SpecRefusal::InvalidCwd,
            }) = &reply
            {
                refused.send(key.clone()).unwrap();
            }
            send(&mut data_server, &reply).await.unwrap();
        }
    });
    let (cancels, mut cancelled) = mpsc::unbounded_channel();
    let control = tokio::spawn(async move {
        while let Ok(poll) = receive::<ControlPoll>(&mut control_peer).await {
            if let Some(ControlReply::ProcessCancelled { key, .. }) = &poll.receipt {
                cancels.send(key.clone()).unwrap();
            }
            send(&mut control_server, &poll).await.unwrap();
            let Ok(turn) = receive::<ControlTurn>(&mut control_server).await else {
                break;
            };
            send(&mut control_peer, &turn).await.unwrap();
        }
    });
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    let process = "refused-register-has-no-cleanup";
    let watcher = crate::app_server::command::observe_cancellation_watcher(process);
    let mut params = parameters(&fixture, &["/bin/true"]);
    params.process_id = Some(process.into());
    params.cwd = fixture.workspaces[0]
        .join("missing-cwd")
        .display()
        .to_string();
    assert_eq!(
        rejected(&connection, 1, Method::CommandExec(params)).await,
        crate::app_server::protocol::ErrorData::EnvironmentUnauthorized
    );
    let key = bounded(refusals.recv()).await.unwrap();
    bounded(bounded(watcher).await.unwrap()).await.unwrap();
    assert!(matches!(
        cancelled.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    let query = ProcessRequest::Query { key: key.clone() };
    let observed = tokio::task::spawn_blocking(move || endpoint.process(&query))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed, ProcessReply::Unknown { key });
    assert!(matches!(
        call(&connection, 2, Method::CommandExec(parameters(&fixture, &["/bin/true"]))).await,
        MethodResult::CommandExec { result } if result.exit_code == Some(0)
    ));
    connection.close();
    assert_eq!(bounded(fixture.host.drain()).await, [] as [String; 0]);
    // The Client gets a genuine protocol Close from the Server on the
    // data route. Wait for it without interrupting physical cleanup.
    assert!(bounded(child.wait()).await.unwrap().success());
    data.abort();
    control.abort();
}

pub(crate) async fn delete_revoke_does_not_pin_residency_or_forget_failure() {
    use crate::local_runtime::session::deletion::SessionDeleteResult;

    let fixture = Fixture::with_session_count(None, 2).await;
    let sibling = fixture.load(1).await.unwrap().unwrap().client();
    let ProxiedClient {
        data: (mut data_peer, mut data_server),
        control: (mut control_peer, mut control_server),
        endpoint: _endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let data = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut data_peer, &mut data_server).await;
    });
    let (seen, pending) = oneshot::channel();
    let (fail, release) = oneshot::channel();
    let control = tokio::spawn(async move {
        while let Ok(poll) = receive::<ControlPoll>(&mut control_peer).await {
            if matches!(poll.receipt, Some(ControlReply::Revoked)) {
                seen.send(()).unwrap();
                release.await.unwrap();
                return;
            }
            send(&mut control_server, &poll).await.unwrap();
            let turn = receive::<ControlTurn>(&mut control_server).await.unwrap();
            send(&mut control_peer, &turn).await.unwrap();
        }
        panic!("the live Client must acknowledge Revoke");
    });
    let session = fixture.sessions[0].id.clone();
    let SessionDeleteResult::Preview { preview } =
        fixture.manager.sessions.delete_preview(&session).await
    else {
        panic!("expected a deletable Session")
    };
    let deleted = tokio::spawn({
        let manager = fixture.manager.clone();
        let session = session.clone();
        async move {
            manager
                .delete_session(&session, &preview.target_revision)
                .await
        }
    });
    bounded(pending).await.unwrap();
    let admitted = tokio::task::spawn_blocking(move || sibling.admit_operation().map(|_| ()));
    // Hold the actual Client receipt while another Session takes the
    // same global registry mutex to admit a native operation.
    tokio::time::timeout(std::time::Duration::from_secs(2), admitted)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!deleted.is_finished());
    fail.send(()).unwrap();
    bounded(control).await.unwrap();
    let SessionDeleteResult::CommittedCleanupPending { record, detail } =
        bounded(deleted).await.unwrap().unwrap()
    else {
        panic!("an unconfirmed revoke must preserve the cleanup workset");
    };
    assert_eq!(record.session_id, session);
    assert_eq!(
        detail.as_deref(),
        Some("previous Session execution domain requires physical settlement")
    );
    assert!(
        fixture
            .manager
            .sessions
            .read_session(&session)
            .await
            .is_err()
    );

    let failures = bounded(fixture.host.drain()).await;
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("Client revocation is unconfirmed"),
        "{failures:?}"
    );
    let _ = bounded(child.wait()).await.unwrap();
    bounded(data).await.unwrap();
}

#[allow(clippy::too_many_lines)] // Keep the capture and Revoke barriers in one scenario.
pub(crate) async fn queued_capture_discard_hands_off_to_binding_revocation() {
    use rustx_execution_protocol::process::{
        OperationId, ProcessProgram, ProcessSpec, ProcessStdin,
    };
    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data: (mut data_peer, mut data_server),
        control: (mut control_peer, mut control_server),
        endpoint,
        mut child,
        _scratch: scratch,
    } = proxied_client(&fixture).await;
    let data = tokio::spawn(async move {
        while let Ok(request) = receive::<ClientRequest>(&mut data_server).await {
            assert!(!matches!(
                request,
                ClientRequest::Process(ProcessRequest::DiscardBashOutput { .. })
            ));
            send(&mut data_peer, &request).await.unwrap();
            let reply = receive::<ClientReply>(&mut data_peer).await.unwrap();
            send(&mut data_server, &reply).await.unwrap();
        }
        let _ = data_peer.shutdown().await;
    });
    let operation = OperationId::new();
    let key = endpoint.process_key(operation).unwrap();
    let spec = ProcessSpec {
        cwd: std::path::PathBuf::new(),
        program: ProcessProgram::Shell {
            command: "printf retained".into(),
            conversation: fixture.sessions[0].active_conversation_id.as_str().into(),
            live_output: true,
        },
        env: std::collections::BTreeMap::new(),
        timeout_ms: 5000,
        pty: None,
        stdin: ProcessStdin::Closed,
        capture: false,
        output_cap: 0,
    };
    let registered = tokio::task::spawn_blocking({
        let endpoint = endpoint.clone();
        let key = key.clone();
        move || {
            let request = ProcessRequest::RegisterAgentBash {
                key,
                fingerprint: spec.fingerprint(),
                spec,
            };
            endpoint.agent_process(
                &request,
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                || Ok(()),
            )
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(registered, ProcessReply::Registered { key: key.clone() });
    let name = format!("{:032x}.output", operation.as_u128());
    let capture = std::fs::read_dir(scratch.path())
        .unwrap()
        .map(|entry| entry.unwrap().path().join(&name))
        .find(|path| path.is_file())
        .expect("registered private capture");
    let (resume_discard, discard_gate) = oneshot::channel();
    let discard = tokio::spawn({
        let endpoint = endpoint.clone();
        let key = key.clone();
        async move {
            discard_gate.await.unwrap();
            endpoint.discard_bash_capture(key).await;
        }
    });
    let (seen, pending) = oneshot::channel();
    let (resume, held) = oneshot::channel();
    let capture_path = capture.clone();
    let control = tokio::spawn(async move {
        let mut gate = Some((seen, held));
        while let Ok(poll) = receive::<ControlPoll>(&mut control_peer).await {
            if matches!(poll.receipt, Some(ControlReply::Revoked)) {
                assert!(
                    !capture.exists(),
                    "physical capture must be gone before Revoke is acknowledged"
                );
                let (seen, held) = gate.take().expect("a single Revoke owns settlement");
                seen.send(()).unwrap();
                held.await.unwrap();
                send(&mut control_server, &poll).await.unwrap();
                // The Client's final poll does not wait for another turn.
                let _ = receive::<ControlTurn>(&mut control_server).await;
                return;
            }
            if send(&mut control_server, &poll).await.is_err() {
                return;
            }
            let Ok(turn) = receive::<ControlTurn>(&mut control_server).await else {
                return;
            };
            send(&mut control_peer, &turn).await.unwrap();
        }
    });
    let exchange = |request| {
        let endpoint = endpoint.clone();
        async move {
            tokio::task::spawn_blocking(move || endpoint.process(&request))
                .await
                .unwrap()
                .unwrap()
        }
    };
    assert!(matches!(
        exchange(ProcessRequest::Start { key: key.clone() }).await,
        ProcessReply::State { .. }
    ));
    let snapshot = bounded(async {
        loop {
            let ProcessReply::State { state, .. } =
                exchange(ProcessRequest::Query { key: key.clone() }).await
            else {
                panic!("the registered Bash operation must have state")
            };
            if let OperationState::Settled(snapshot) = state {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(snapshot.disposition, SettledDisposition::Known);
    assert_eq!(
        exchange(ProcessRequest::ReleaseOutput {
            key: key.clone(),
            final_sequence: snapshot.final_sequence,
        })
        .await,
        ProcessReply::OutputReleased { key }
    );
    assert_eq!(std::fs::read(capture_path).unwrap(), b"retained");

    let drain = tokio::spawn({
        let host = fixture.host.clone();
        async move { host.drain().await }
    });
    bounded(pending).await.unwrap();
    resume_discard.send(()).unwrap();
    bounded(discard).await.unwrap();
    assert!(!drain.is_finished(), "the Client receipt is still parked");
    resume.send(()).unwrap();
    assert_eq!(bounded(drain).await.unwrap(), [] as [String; 0]);
    assert!(bounded(child.wait()).await.unwrap().success());
    bounded(data).await.unwrap();
    bounded(control).await.unwrap();
}

pub(crate) enum UnavailableBashCapture {
    ClientReported { refuse_first_discard: bool },
    QueryBusy,
}

pub(crate) enum ForegroundBashExecution {
    Direct,
    PrivateStage,
}

#[allow(clippy::too_many_lines)] // Keep the native invocation and its physical cleanup trace together.
pub(crate) async fn foreground_bash_discards_unavailable_capture_after_journal_release(
    execution: ForegroundBashExecution,
    unavailable: UnavailableBashCapture,
) {
    use crate::runtime::identity::ToolCallId;
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolRegistry};
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::types::{
        ManagedOutputContinuation, ToolInvocation, ToolInvocationId, ToolInvocationMode,
        ToolInvocationPolicy, ToolProgress,
    };
    use crate::tools::workspace::Workspace;
    use rustx_execution_protocol::process::{BashCaptureState, ProcessProgram};

    struct NoProgress;
    impl ProgressReporter for NoProgress {
        fn report(&self, _progress: ToolProgress) {}
    }

    let query_busy = matches!(unavailable, UnavailableBashCapture::QueryBusy);
    let refuse_first_discard = matches!(
        unavailable,
        UnavailableBashCapture::ClientReported {
            refuse_first_discard: true
        }
    );
    let private_stage = matches!(execution, ForegroundBashExecution::PrivateStage);
    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control: (mut client_control, mut server_control),
        endpoint,
        mut child,
        _scratch: scratch,
    } = proxied_client(&fixture).await;
    let control = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut client_control, &mut server_control).await;
    });
    if query_busy {
        endpoint.expire_bash_capture_query_for_test();
    }
    // The Client still uses its scratch root while shutting down.
    let client_scratch = scratch.path().to_path_buf();
    let (file_retained, retained_file) = oneshot::channel();
    let (discard_received, discarded) = oneshot::channel();
    let data = tokio::spawn(async move {
        let mut file_retained = Some(file_retained);
        let mut discard_received = Some(discard_received);
        let mut trace = Vec::new();
        let mut path = None;
        let mut operation = None;
        while let Ok(request) = receive::<ClientRequest>(&mut server_data).await {
            if query_busy {
                assert!(!matches!(
                    request,
                    ClientRequest::Process(ProcessRequest::BashCapture { .. })
                ));
            }
            if let ClientRequest::Process(ProcessRequest::DiscardBashOutput { key }) = &request {
                assert_eq!(Some(key), operation.as_ref());
                assert!(
                    discard_received.is_some(),
                    "discard started again after success"
                );
                if refuse_first_discard && trace == ["capture", "release"] {
                    assert!(path.as_deref().is_some_and(Path::is_file));
                    trace.push("discard refused");
                    let reply = ClientReply::Process(ProcessReply::Conflict { key: key.clone() });
                    send(&mut server_data, &reply).await.unwrap();
                    continue;
                }
            }
            send(&mut client_data, &request).await.unwrap();
            let mut reply = receive::<ClientReply>(&mut client_data).await.unwrap();
            match (&request, &mut reply) {
                (
                    ClientRequest::Process(ProcessRequest::RegisterAgentBash { key, spec, .. }),
                    ClientReply::Process(ProcessReply::Registered { key: admitted }),
                ) if operation.is_none() => {
                    assert_eq!(key, admitted);
                    assert!(matches!(
                        spec.program,
                        ProcessProgram::Shell {
                            live_output,
                            ..
                        } if live_output == private_stage
                    ));
                    assert!(operation.replace(key.clone()).is_none());
                }
                (
                    _,
                    ClientReply::Process(ProcessReply::BashCapture {
                        key,
                        capture: Some(capture),
                    }),
                ) => {
                    assert_eq!(Some(&*key), operation.as_ref());
                    assert_eq!(capture.output, BashCaptureState::Complete);
                    // The Client's own regression injects the append/rollback fault.
                    capture.output = BashCaptureState::Unavailable;
                    capture.total = None;
                    capture.truncated = true;
                    trace.push("capture");
                }
                (
                    ClientRequest::Process(ProcessRequest::ReleaseOutput { key, .. }),
                    ClientReply::Process(ProcessReply::OutputReleased { key: released }),
                ) if Some(key) == operation.as_ref() => {
                    assert_eq!(key, released);
                    let name = format!("{:032x}.output", key.operation.as_u128());
                    path = std::fs::read_dir(&client_scratch)
                        .unwrap()
                        .map(|entry| entry.unwrap().path().join(&name))
                        .find(|path| path.is_file());
                    assert_eq!(
                        std::fs::read(path.as_ref().unwrap()).unwrap(),
                        vec![b' '; 65_536]
                    );
                    file_retained
                        .take()
                        .unwrap()
                        .send(path.clone().unwrap())
                        .unwrap();
                    trace.push("release");
                }
                (
                    ClientRequest::Process(ProcessRequest::DiscardBashOutput { key }),
                    ClientReply::Process(ProcessReply::OutputReleased { key: released }),
                ) => {
                    assert_eq!(key, released);
                    assert!(!path.as_deref().unwrap().exists());
                    trace.push("discard confirmed");
                    discard_received.take().unwrap().send(()).unwrap();
                }
                _ => {}
            }
            send(&mut server_data, &reply).await.unwrap();
        }
        let _ = client_data.shutdown().await;
        trace
    });

    let session = &fixture.sessions[0];
    let conversation = &session.active_conversation_id;
    let mut workspace = Workspace::new(endpoint.workspace()).unwrap();
    workspace.bind_execution_scope(Some(fixture.manager.execution_scope(&session.id)));
    let output = tempfile::tempdir().unwrap();
    let artifacts = ArtifactStore::new(conversation.clone(), output.path()).unwrap();
    let managed =
        ManagedToolOutput::new(conversation.clone(), output.path().join("managed")).unwrap();
    let environment = crate::tools::environment::ToolEnvironment::new();
    let bash =
        crate::tools::native::subagent_child_definition("bash", ToolInvocationPolicy::default())
            .unwrap();
    let invocation = ToolInvocation {
        id: ToolInvocationId::Agent {
            call_id: ToolCallId::new("foreground-capture-cleanup"),
        },
        tool_id: bash.id.clone(),
        tool_name: bash.name.clone(),
        mode: if private_stage {
            ToolInvocationMode::Background
        } else {
            ToolInvocationMode::Foreground
        },
        arguments: serde_json::json!({
            "description": "Generate foreground output",
            "command": "printf '%65536s' ''",
        }),
    };
    let cancellation = detached_cancellation(crate::runtime::CancellationSignal::new());
    let result = if private_stage {
        use crate::runtime::inbound::ConversationInboundMailbox;
        use crate::tools::background::{BackgroundResources, ConversationBackgroundRegistry};

        let prepared = crate::tools::native::PreparedBash::register(
            &invocation,
            &workspace,
            conversation,
            &environment,
            &cancellation,
        )
        .await
        .unwrap();
        let capture_owner = prepared.capture_owner().clone();
        let registry = ConversationBackgroundRegistry::new(
            conversation.clone(),
            BackgroundResources {
                mailbox: ConversationInboundMailbox::new(conversation.clone()),
                workspace,
                artifacts,
                tool_output: managed,
                clock: Arc::new(crate::runtime::SystemClock),
                event_sink: None,
            },
        );
        let mut stage = registry
            .stage_private_bash(
                &invocation,
                Arc::new(prepared),
                environment,
                crate::tools::mcp::McpRuntimeLeaseSet::default(),
                cancellation,
            )
            .unwrap();
        let result = bounded(async {
            loop {
                if let Some(result) = stage.take_result() {
                    break result;
                }
                stage.wait_event().await;
            }
        })
        .await;
        let path = bounded(retained_file).await.unwrap();
        if query_busy {
            // While the stage could still be promoted, readable output survives.
            assert_eq!(std::fs::read(path).unwrap(), vec![b' '; 65_536]);
        } else {
            // The driver already received a successful discard before handoff.
            assert!(!path.exists());
        }
        let result = stage.release_finished(result);
        if !query_busy {
            // The shared owner rejects another request before queuing admission.
            assert!(capture_owner.discard().is_none());
        }
        assert_eq!(registry.listing(false, 16).matched, 0);
        result
    } else {
        let context = ToolExecutionContext::new(
            conversation,
            None,
            cancellation,
            &workspace,
            &NoProgress,
            &artifacts,
            &managed,
            &environment,
        );
        let mut registry = ToolRegistry::new();
        crate::tools::native::register_subagent_child_tools(
            &mut registry,
            std::slice::from_ref(&bash),
        )
        .unwrap();
        let executor = registry.executor(&bash.id);
        let result = bounded(executor.start(invocation, context).completion).await;
        bounded(retained_file).await.unwrap();
        result
    };
    assert!(matches!(
        result.managed_output.as_deref(),
        Some(ManagedOutputContinuation::Unavailable { .. })
    ));
    bounded(discarded).await.unwrap();
    let handle = endpoint
        .control()
        .expect("the binding should survive a confirmed discard retry");
    bounded(tokio::task::spawn_blocking(move || handle.health()))
        .await
        .unwrap()
        .unwrap();
    if query_busy || private_stage {
        let conversation = conversation.as_str().to_owned();
        bounded(tokio::task::spawn_blocking(move || {
            assert_bash_capture_quota_recovered(&endpoint, conversation);
        }))
        .await
        .unwrap();
    }
    grant(&fixture);
    let connection = AppServerConnection::new(fixture.host.clone());
    initialize(&connection).await;
    assert!(matches!(
        bounded(call(&connection, 1, Method::CommandExec(parameters(&fixture, &["/bin/true"])))).await,
        MethodResult::CommandExec { result } if result.status == ExecStatus::Completed && result.exit_code == Some(0)
    ));
    connection.close();
    assert_eq!(bounded(fixture.host.drain()).await, [] as [String; 0]);
    assert!(bounded(child.wait()).await.unwrap().success());
    let expected: &[_] = if query_busy {
        &["release", "discard confirmed"]
    } else if refuse_first_discard {
        &["capture", "release", "discard refused", "discard confirmed"]
    } else {
        &["capture", "release", "discard confirmed"]
    };
    assert_eq!(bounded(data).await.unwrap(), expected);
    bounded(control).await.unwrap();
}

fn assert_bash_capture_quota_recovered(
    endpoint: &crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint,
    conversation: String,
) {
    use rustx_execution_protocol::process::{
        MAX_BASH_CAPTURES, OperationId, ProcessProgram, ProcessSpec, ProcessStdin,
    };
    use rustx_execution_protocol::transport::PAGE_REQUEST_TIMEOUT;
    use std::time::Instant;

    let spec = ProcessSpec {
        cwd: std::path::PathBuf::new(),
        program: ProcessProgram::Shell {
            command: "exit 0".into(),
            conversation,
            live_output: true,
        },
        env: std::collections::BTreeMap::new(),
        timeout_ms: 0,
        pty: None,
        stdin: ProcessStdin::Closed,
        capture: false,
        output_cap: 0,
    };
    let fingerprint = spec.fingerprint();
    let control = endpoint.control().unwrap();
    for _ in 0..MAX_BASH_CAPTURES {
        let key = endpoint.process_key(OperationId::new()).unwrap();
        let registration = ProcessRequest::RegisterAgentBash {
            key: key.clone(),
            fingerprint,
            spec: spec.clone(),
        };
        assert_eq!(
            endpoint
                .agent_process(&registration, Instant::now() + PAGE_REQUEST_TIMEOUT, || Ok(
                    ()
                ))
                .unwrap(),
            ProcessReply::Registered { key: key.clone() },
        );
        // Cancellation retires the registration and leaves its capture charged.
        assert_eq!(
            control.cancel_process(&key).unwrap(),
            ProcessCancelDisposition::CancelledBeforeStart
        );
    }
}

fn park_revoked_receipt(
    (mut client, mut server): (UnixStream, UnixStream),
) -> (
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (seen, pending) = oneshot::channel();
    let (resume, held) = oneshot::channel();
    let relay = tokio::spawn(async move {
        loop {
            let poll: ControlPoll = receive(&mut client).await.unwrap();
            if matches!(poll.receipt, Some(ControlReply::Revoked)) {
                seen.send(()).unwrap();
                held.await.unwrap();
                send(&mut server, &poll).await.unwrap();
                let _ = receive::<ControlTurn>(&mut server).await;
                return;
            }
            send(&mut server, &poll).await.unwrap();
            let turn: ControlTurn = receive(&mut server).await.unwrap();
            send(&mut client, &turn).await.unwrap();
        }
    });
    (pending, resume, relay)
}

async fn wait_for_data_capacity(
    endpoint: &crate::local_runtime::session_runtime_manager::LocalExecutionEndpoint,
    available: usize,
) {
    bounded(async {
        while endpoint.available_data_for_test() != available {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

fn detached_cancellation(
    signal: crate::runtime::CancellationSignal,
) -> crate::runtime::ExecutionCancellation {
    crate::runtime::ExecutionCancellation::detached(
        signal,
        crate::runtime::types::CancellationReason::UserRequested,
    )
}

#[cfg(target_os = "linux")]
pub(crate) async fn client_sigkill_keeps_other_sessions_and_history_available() {
    use std::os::unix::process::ExitStatusExt as _;

    let fixture = Fixture::with_session_count(None, 3).await;
    grant(&fixture);
    let mut killed = fixture.start_execution(0).await;
    let mut independent = fixture.start_execution(1).await;
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let script = format!("exec 3<>/dev/tcp/127.0.0.1/{port}; printf ready >&3; read -r -u 3");
    let mut command = parameters(&fixture, &["/bin/bash", "-c", &script]);
    command.timeout_ms = Some(60_000);
    let interrupted = tokio::spawn({
        let connection = connection.clone();
        async move { rejected(&connection, 1, Method::CommandExec(command)).await }
    });
    let (mut business, _) = bounded(listener.accept()).await.unwrap();
    let mut ready = [0; 5];
    bounded(business.read_exact(&mut ready)).await.unwrap();
    assert_eq!(&ready, b"ready");
    killed.start_kill().unwrap();
    assert_eq!(
        bounded(killed.wait()).await.unwrap().signal(),
        Some(nix::libc::SIGKILL)
    );

    // These requests are independent of any result or cleanup evidence
    // from the dead Client's binding.
    let mut command = parameters(&fixture, &["/bin/sh", "-c", "printf alive"]);
    command.session_id = fixture.sessions[1].id.clone();
    command.cwd = fixture.workspaces[1].display().to_string();
    let MethodResult::CommandExec { result } =
        call(&connection, 2, Method::CommandExec(command)).await
    else {
        panic!("the independent Session must execute");
    };
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result.stdout_base64)
            .unwrap(),
        b"alive"
    );
    let cold_session = &fixture.sessions[2].id;
    assert!(fixture.manager.execution_endpoint(cold_session).is_err());
    let MethodResult::Session { session } = call(
        &connection,
        3,
        Method::SessionRead {
            session_id: cold_session.clone(),
        },
    )
    .await
    else {
        panic!("the unbound Session must remain readable");
    };
    assert_eq!(&session.id, cold_session);
    assert!(fixture.manager.execution_endpoint(cold_session).is_err());
    assert_eq!(fixture.provider.request_bodies(), [] as [String; 0]);

    assert_eq!(bounded(business.read(&mut ready)).await.unwrap(), 0);
    assert!(matches!(
        bounded(interrupted).await.unwrap(),
        ErrorData::OutcomeUnknown | ErrorData::ExecutionUnavailable
    ));
    connection.close();
    let unsettled = bounded(fixture.host.drain()).await;
    assert_eq!(unsettled.len(), 1);
    assert!(unsettled[0].contains("unconfirmed"));
    assert!(bounded(independent.wait()).await.unwrap().success());
}

pub(crate) async fn lost_input_acceptance_reports_unknown_and_retires_the_binding() {
    const INPUT: &[u8; 15] = b"already written";

    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control: (mut client_control, mut server_control),
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let (accepted, acceptance_parked) = oneshot::channel();
    let (disconnect, cut) = oneshot::channel();
    let data = tokio::spawn(async move {
        loop {
            let request = receive::<ClientRequest>(&mut server_data).await.unwrap();
            send(&mut client_data, &request).await.unwrap();
            let reply = receive::<ClientReply>(&mut client_data).await.unwrap();
            if let ClientRequest::Process(ProcessRequest::WriteStdin {
                key,
                sequence,
                delta_base64,
                close,
            }) = request
            {
                assert_eq!(sequence.get(), 1);
                assert!(!close);
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(delta_base64)
                        .unwrap(),
                    INPUT
                );
                assert_eq!(
                    reply,
                    ClientReply::Process(ProcessReply::InputAccepted { key, sequence })
                );
                break;
            }
            send(&mut server_data, &reply).await.unwrap();
        }
        accepted.send(()).unwrap();
        cut.await.unwrap();

        server_data.shutdown().await.unwrap();
        assert_eq!(
            receive::<ClientRequest>(&mut server_data)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof,
            "the Server retires the pair without replay or receipt polling"
        );
    });
    let control = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut client_control, &mut server_control).await;
    });
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    // Cat echoes on an independent socket while the Server's data lane
    // is blocked waiting for InputAccepted.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let script = format!("exec > /dev/tcp/127.0.0.1/{port}; exec /bin/cat");
    let id = "lost-initial-input-acceptance";
    let mut params = parameters(&fixture, &["/bin/bash", "-c", &script]);
    params.process_id = Some(id.into());
    params.stream_stdin = true;
    let command = tokio::spawn({
        let connection = connection.clone();
        async move { rejected(&connection, 1, Method::CommandExec(params)).await }
    });
    let (mut business, _) = bounded(listener.accept()).await.unwrap();

    let write = |delta_base64| {
        Method::CommandWrite(WriteParams {
            process_id: id.into(),
            delta_base64: Some(delta_base64),
            close_stdin: false,
        })
    };
    assert_eq!(
        rejected(&connection, 2, write("!".into())).await,
        ErrorData::InvalidParams,
        "local validation proves this input never reached the Client"
    );
    let input = tokio::spawn({
        let connection = connection.clone();
        let request = write(base64::engine::general_purpose::STANDARD.encode(INPUT));
        async move { rejected(&connection, 3, request).await }
    });
    bounded(acceptance_parked).await.unwrap();
    let mut echoed = [0; INPUT.len()];
    bounded(business.read_exact(&mut echoed)).await.unwrap();
    assert_eq!(&echoed, INPUT);
    assert!(!input.is_finished());
    assert!(endpoint.control().is_some());
    disconnect.send(()).unwrap();
    assert_eq!(bounded(input).await.unwrap(), ErrorData::OutcomeUnknown);
    bounded(data).await.unwrap();
    assert!(endpoint.control().is_none());
    bounded(command).await.unwrap();
    assert!(
        bounded(fixture.host.drain())
            .await
            .iter()
            .any(|error| error.contains("unconfirmed"))
    );
    bounded(control).await.unwrap();
    bounded(child.wait()).await.unwrap();
    let mut extra = Vec::new();
    assert_eq!(bounded(business.read_to_end(&mut extra)).await.unwrap(), 0);
}

pub(crate) async fn expired_input_cleanup_settles_the_binding(revoking: bool) {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control,
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let data = tokio::spawn(async move {
        let mut accepted = 0;
        while let Ok(request) = receive::<ClientRequest>(&mut server_data).await {
            assert!(
                !matches!(
                    request,
                    ClientRequest::Process(ProcessRequest::InputReceipt { .. })
                ),
                "the expired cleanup must fail before sending a frame"
            );
            send(&mut client_data, &request).await.unwrap();
            let reply = receive::<ClientReply>(&mut client_data).await.unwrap();
            if matches!(
                reply,
                ClientReply::Process(ProcessReply::InputAccepted { .. })
            ) {
                accepted += 1;
            }
            send(&mut server_data, &reply).await.unwrap();
        }
        accepted
    });
    let (revocation, control) = if revoking {
        let (revoked, deliver_revoked, relay) = park_revoked_receipt(control);
        (Some((revoked, deliver_revoked)), relay)
    } else {
        let (mut client, mut server) = control;
        (
            None,
            tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
            }),
        )
    };
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let process_id = if revoking {
        "expired-input-during-revoke"
    } else {
        "expired-input-open-binding"
    };
    let (collector_parked, resume_collector) =
        crate::app_server::command::hold_expired_input_cleanup(process_id);
    let mut params = parameters(&fixture, &["/bin/cat"]);
    params.process_id = Some(process_id.into());
    params.stream_stdin = true;
    params.stream_stdout_stderr = true;
    let driver = connection.clone();
    let mut command = tokio::spawn(async move {
        driver
            .handle_request(Request {
                jsonrpc: JsonRpcVersion::V2,
                id: RequestId::Integer(1),
                call: Method::CommandExec(params),
            })
            .await
    });
    admit_or_fail(&connection, process_id, &mut command).await;
    let input = tokio::spawn({
        let connection = connection.clone();
        async move {
            rejected(
                &connection,
                2,
                Method::CommandWrite(WriteParams {
                    process_id: process_id.into(),
                    delta_base64: Some(
                        base64::engine::general_purpose::STANDARD.encode(b"accepted"),
                    ),
                    close_stdin: false,
                }),
            )
            .await
        }
    });
    bounded(collector_parked).await.unwrap();
    let mut echoed = Vec::new();
    while echoed.len() < b"accepted".len() {
        echoed.extend(output(&connection, process_id).await);
    }
    assert_eq!(
        echoed, b"accepted",
        "the process consumed the complete input"
    );
    let (data_held, holding_data) = oneshot::channel();
    let (release_data, contested) = std::sync::mpsc::channel();
    let lock = tokio::task::spawn_blocking({
        let endpoint = endpoint.clone();
        move || {
            endpoint.hold_data_exchange_for_test(|| {
                let _ = data_held.send(());
                let _ = contested.recv();
            })
        }
    });
    bounded(holding_data).await.unwrap();
    let terminal = if let Some((revoked, deliver_revoked)) = revocation {
        let host = fixture.host.clone();
        let drain = tokio::spawn(async move { host.drain().await });
        bounded(revoked).await.unwrap();
        Some((drain, deliver_revoked))
    } else {
        assert!(endpoint.control().is_some(), "the binding starts live");
        None
    };
    resume_collector.send(()).unwrap();
    assert_eq!(bounded(input).await.unwrap(), ErrorData::OutcomeUnknown);
    assert_eq!(
        endpoint.control().is_some(),
        revoking,
        "cleanup must respect the terminal owner"
    );
    release_data.send(()).unwrap();
    bounded(lock).await.unwrap().unwrap();
    if let Some((drain, deliver_revoked)) = terminal {
        assert!(
            !drain.is_finished(),
            "the actual Revoke receipt is still parked"
        );
        deliver_revoked.send(()).unwrap();
        assert_eq!(bounded(drain).await.unwrap(), [] as [String; 0]);
        assert!(bounded(child.wait()).await.unwrap().success());
    } else {
        assert!(
            bounded(fixture.host.drain())
                .await
                .iter()
                .any(|error| error.contains("unconfirmed"))
        );
        bounded(child.wait()).await.unwrap();
    }
    assert!(matches!(
        bounded(command).await.unwrap(),
        Response::Failure(_)
    ));
    assert_eq!(
        bounded(data).await.unwrap(),
        1,
        "one real Client input admission"
    );
    bounded(control).await.unwrap();
}

pub(crate) async fn failed_input_receipt_after_poll_expiry_preserves_the_binding() {
    use crate::app_server::command::{hold_expired_input_poll, hold_start_after_gate};
    use rustx_execution_protocol::process::InputOutcome;

    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control: (mut client_control, mut server_control),
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let data = tokio::spawn(async move {
        let mut receipts = Vec::new();
        while let Ok(request) = receive::<ClientRequest>(&mut server_data).await {
            send(&mut client_data, &request).await.unwrap();
            let reply = receive::<ClientReply>(&mut client_data).await.unwrap();
            if let ClientRequest::Process(ProcessRequest::InputReceipt {
                abandon_pending, ..
            }) = request
            {
                let ClientReply::Process(ProcessReply::InputReceipt {
                    sequence, status, ..
                }) = reply
                else {
                    panic!("expected an exact input receipt, got {reply:?}");
                };
                assert_eq!(sequence.get(), 1);
                receipts.push((abandon_pending, status));
            }
            send(&mut server_data, &reply).await.unwrap();
        }
        receipts
    });
    let control = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut client_control, &mut server_control).await;
    });
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let id = "failed-receipt-after-poll-expiry";
    let start = hold_start_after_gate(id);
    let (collector_parked, resume_collector) = hold_expired_input_poll(id);
    let mut params = parameters(&fixture, &["/bin/cat"]);
    params.process_id = Some(id.into());
    params.stream_stdin = true;
    let command = tokio::spawn({
        let connection = connection.clone();
        async move { call(&connection, 1, Method::CommandExec(params)).await }
    });
    let key = bounded(start.key()).await;
    let input = tokio::spawn({
        let connection = connection.clone();
        async move {
            rejected(
                &connection,
                2,
                Method::CommandWrite(WriteParams {
                    process_id: id.into(),
                    delta_base64: None,
                    close_stdin: true,
                }),
            )
            .await
        }
    });
    bounded(collector_parked).await.unwrap();
    let cancel = endpoint.control().unwrap();
    assert_eq!(
        bounded(tokio::task::spawn_blocking(
            move || cancel.cancel_process(&key)
        ))
        .await
        .unwrap()
        .unwrap(),
        ProcessCancelDisposition::CancelledBeforeStart
    );
    start.release();
    let MethodResult::CommandExec { result } = bounded(command).await.unwrap() else {
        panic!("the cancelled command must settle");
    };
    assert_eq!(result.status, ExecStatus::Terminated);
    resume_collector.send(()).unwrap();
    assert_eq!(bounded(input).await.unwrap(), ErrorData::OutcomeUnknown);
    assert!(
        endpoint.control().is_some(),
        "the claimed Failed receipt settles ownership"
    );
    assert!(matches!(
        call(&connection, 3, Method::CommandExec(parameters(&fixture, &["/bin/true"]))).await,
        MethodResult::CommandExec { result } if result.exit_code == Some(0)
    ));
    connection.close();
    assert_eq!(bounded(fixture.host.drain()).await, [] as [String; 0]);
    assert_eq!(bounded(data).await.unwrap(), [(true, InputOutcome::Failed)]);
    bounded(control).await.unwrap();
    assert!(bounded(child.wait()).await.unwrap().success());
}

pub(crate) async fn queued_workspace_release_hands_off_to_binding_revocation() {
    use crate::runtime::identity::AttemptId;
    use crate::tools::workspace::Workspace;
    use crate::workspace_changes::{ChangeRecording, ChangesStore};
    use rustx_execution_protocol::MAX_DATA_RECORDS_PER_CONNECTION;
    use rustx_execution_protocol::changes::{ChangeReply, ChangeRequest};

    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control,
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let data = tokio::spawn(async move {
        let begin = receive::<ClientRequest>(&mut server_data).await.unwrap();
        assert!(matches!(
            begin,
            ClientRequest::WorkspaceChanges(ChangeRequest::Begin { .. })
        ));
        send(&mut client_data, &begin).await.unwrap();
        let reply = receive::<ClientReply>(&mut client_data).await.unwrap();
        assert_eq!(reply, ClientReply::WorkspaceChanges(ChangeReply::Accepted));
        send(&mut server_data, &reply).await.unwrap();
        assert!(
            receive::<ClientRequest>(&mut server_data).await.is_err(),
            "Release crossed after Revoke"
        );
    });
    let (revoked, deliver_revoked, control) = park_revoked_receipt(control);
    let session = &fixture.sessions[0].id;
    let mut workspace = Workspace::new(endpoint.workspace()).unwrap();
    workspace.bind_execution_scope(Some(fixture.manager.execution_scope(session)));
    let recording = ChangeRecording::start(
        AttemptId::new("workspace-release-under-revoke"),
        &workspace,
        Arc::new(ChangesStore::default()),
        crate::runtime::CancellationSignal::new(),
    );
    let released = bounded(recording.observe_release_for_test()).await;
    let writer = endpoint.writer_for_test().acquire_owned().await.unwrap();
    drop(recording);
    wait_for_data_capacity(&endpoint, MAX_DATA_RECORDS_PER_CONNECTION - 1).await;
    let host = fixture.host.clone();
    let drain = tokio::spawn(async move { host.drain().await });
    bounded(revoked).await.unwrap();
    drop(writer);
    bounded(released).await.unwrap();
    assert!(
        !drain.is_finished(),
        "the Client's Revoke receipt is still parked"
    );
    deliver_revoked.send(()).unwrap();
    assert_eq!(bounded(drain).await.unwrap(), [] as [String; 0]);
    assert!(bounded(child.wait()).await.unwrap().success());
    bounded(data).await.unwrap();
    bounded(control).await.unwrap();
}

pub(crate) async fn cancelled_preparation_hands_off_to_binding_revocation() {
    use rustx_execution_protocol::MAX_DATA_RECORDS_PER_CONNECTION;

    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control,
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let (seen, prepared) = oneshot::channel();
    let (reply, allow_reply) = oneshot::channel();
    let data = tokio::spawn(async move {
        let request = receive::<ClientRequest>(&mut server_data).await.unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        send(&mut client_data, &request).await.unwrap();
        let response = receive::<ClientReply>(&mut client_data).await.unwrap();
        let ClientReply::WritePrepared { ticket, .. } = response else {
            panic!("the physical Client must prepare the ticket");
        };
        seen.send(ticket).unwrap();
        allow_reply.await.unwrap();
        send(&mut server_data, &response).await.unwrap();
        assert!(
            receive::<ClientRequest>(&mut server_data).await.is_err(),
            "terminal settlement owns the ticket without another data request"
        );
    });
    let (revoke, release_revoke, control) = park_revoked_receipt(control);
    let session = &fixture.sessions[0].id;
    let scope = fixture.manager.execution_scope(session);
    let signal = crate::runtime::CancellationSignal::new();
    let prepare = tokio::spawn({
        let cancellation = detached_cancellation(signal.clone());
        async move {
            scope
                .prepare_write(
                    std::path::Path::new("unpublished"),
                    b"data".to_vec(),
                    &cancellation,
                )
                .await
        }
    });
    let ticket = bounded(prepared).await.unwrap();
    let cleanup_done = endpoint.watch_prepared_cleanup_for_test(ticket);
    let mut writer = Box::pin(endpoint.writer_for_test().acquire_owned());
    assert!(futures_util::poll!(&mut writer).is_pending());
    signal.cancel();
    reply.send(()).unwrap();
    let writer = bounded(writer).await.unwrap();
    assert_eq!(
        bounded(prepare).await.unwrap().unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    wait_for_data_capacity(&endpoint, MAX_DATA_RECORDS_PER_CONNECTION - 1).await;

    let host = fixture.host.clone();
    let drain = tokio::spawn(async move { host.drain().await });
    bounded(revoke).await.unwrap();
    drop(writer);
    wait_for_data_capacity(&endpoint, MAX_DATA_RECORDS_PER_CONNECTION).await;
    bounded(cleanup_done).await.unwrap();
    assert!(!drain.is_finished(), "the real Client receipt is parked");
    release_revoke.send(()).unwrap();
    assert_eq!(bounded(drain).await.unwrap(), [] as [String; 0]);
    assert!(bounded(child.wait()).await.unwrap().success());
    bounded(data).await.unwrap();
    bounded(control).await.unwrap();
    assert!(!endpoint.workspace().join("unpublished").exists());
}

#[allow(clippy::too_many_lines)] // Order both physical receipts around the actual watcher decision.
pub(crate) async fn unknown_mutation_cancel_hands_off_to_binding_revocation() {
    use rustx_execution_protocol::CancelDisposition;
    use std::path::Path;

    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control,
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let (published, written) = oneshot::channel();
    let (deliver_written, held) = oneshot::channel();
    let data = tokio::spawn(async move {
        let prepare = receive::<ClientRequest>(&mut server_data).await.unwrap();
        assert!(matches!(prepare, ClientRequest::PrepareWrite { .. }));
        send(&mut client_data, &prepare).await.unwrap();
        let response = receive::<ClientReply>(&mut client_data).await.unwrap();
        let ClientReply::WritePrepared { ticket, .. } = response else {
            panic!("the physical Client must prepare the ticket");
        };
        send(&mut server_data, &response).await.unwrap();
        let commit = receive::<ClientRequest>(&mut server_data).await.unwrap();
        assert_eq!(commit, ClientRequest::CommitWrite { ticket });
        send(&mut client_data, &commit).await.unwrap();
        let response = receive::<ClientReply>(&mut client_data).await.unwrap();
        assert!(
            matches!(response, ClientReply::Written { ticket: echoed, .. } if echoed == ticket)
        );
        published.send(()).unwrap();
        held.await.unwrap();
        send(&mut server_data, &response).await.unwrap();
        assert!(
            receive::<ClientRequest>(&mut server_data).await.is_err(),
            "the mutation must not be replayed"
        );
    });
    let (revoke, deliver_revoke, control) = park_revoked_receipt(control);
    let scope = fixture.manager.execution_scope(&fixture.sessions[0].id);
    let signal = crate::runtime::CancellationSignal::new();
    let cancellation = detached_cancellation(signal.clone());
    let relative = Path::new("already-published");
    let prepared = scope
        .prepare_write(relative, b"committed once".to_vec(), &cancellation)
        .await
        .unwrap();
    let gate = endpoint.hold_cancel_mutation_fence_for_test(prepared.ticket);
    let commit = tokio::spawn(async move { scope.commit_prepared(&prepared, &cancellation).await });
    bounded(written).await.unwrap();
    let path = endpoint.workspace().join(relative);
    assert_eq!(std::fs::read(&path).unwrap(), b"committed once");
    signal.cancel();
    assert_eq!(
        bounded(gate.decision_ready).await.unwrap(),
        Some(CancelDisposition::Unknown)
    );

    let host = fixture.host.clone();
    let drain = tokio::spawn(async move { host.drain().await });
    bounded(revoke).await.unwrap();
    gate.resume.send(()).unwrap();
    bounded(gate.decision_done).await.unwrap();
    assert!(
        !commit.is_finished(),
        "the original Written receipt is still parked"
    );
    assert!(
        !drain.is_finished(),
        "the physical Revoked receipt is still parked"
    );
    deliver_written.send(()).unwrap();
    let metadata = bounded(commit).await.unwrap().unwrap();
    assert_eq!(metadata.size, 14);
    assert_eq!(std::fs::read(&path).unwrap(), b"committed once");
    deliver_revoke.send(()).unwrap();
    assert_eq!(bounded(drain).await.unwrap(), [] as [String; 0]);
    assert!(bounded(child.wait()).await.unwrap().success());
    bounded(data).await.unwrap();
    bounded(control).await.unwrap();
}

#[allow(clippy::too_many_lines)] // Keep delegated ticket and capture ownership in the same wire scenario.
pub(crate) async fn child_relay_joins_the_physical_binding_revocation() {
    use crate::local_runtime::session_runtime_manager::ChildExecutionRelay;
    use crate::runtime::subagent::ResolvedSubagentTool;
    use crate::tools::types::ToolInvocationPolicy;
    use rustx_execution_protocol::process::{
        OperationId, ProcessProgram, ProcessSpec, ProcessStdin,
    };

    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control,
        endpoint,
        mut child,
        _scratch: scratch,
    } = proxied_client(&fixture).await;
    let data = tokio::spawn(async move {
        while let Ok(request) = receive::<ClientRequest>(&mut server_data).await {
            assert!(!matches!(
                request,
                ClientRequest::AbortWrite { .. }
                    | ClientRequest::CommitWrite { .. }
                    | ClientRequest::Process(ProcessRequest::DiscardBashOutput { .. })
            ));
            send(&mut client_data, &request).await.unwrap();
            let response = receive::<ClientReply>(&mut client_data).await.unwrap();
            send(&mut server_data, &response).await.unwrap();
        }
        let _ = client_data.shutdown().await;
    });
    let (revoke, release_revoke, control) = park_revoked_receipt(control);
    let session = &fixture.sessions[0];
    let conversation = &session.active_conversation_id;
    let tools = ["write", "bash"].map(|name| {
        let definition =
            crate::tools::native::subagent_child_definition(name, ToolInvocationPolicy::default())
                .unwrap();
        ResolvedSubagentTool::Builtin {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition,
        }
    });
    let relay = ChildExecutionRelay::create(
        &fixture.manager.execution_scope(&session.id),
        endpoint.workspace(),
        conversation,
        &tools,
        None,
    )
    .unwrap();
    let grant = relay.grant();
    let (parent_startup, child_startup) = UnixStream::pair().unwrap();
    relay.transfer(&parent_startup).await.unwrap();
    let scope = grant
        .open(&session.id, endpoint.workspace(), &child_startup)
        .await
        .unwrap();
    let reaped = relay.start().unwrap();
    let cancellation = detached_cancellation(crate::runtime::CancellationSignal::new());
    let _ticket = scope
        .prepare_write(
            std::path::Path::new("child-unpublished"),
            b"data".to_vec(),
            &cancellation,
        )
        .await
        .unwrap();
    let process = scope.endpoint().unwrap();
    let operation = OperationId::new();
    let key = process.process_key(operation).unwrap();
    let spec = ProcessSpec {
        cwd: std::path::PathBuf::new(),
        program: ProcessProgram::Shell {
            command: "touch child-started".into(),
            conversation: conversation.as_str().into(),
            live_output: true,
        },
        env: std::collections::BTreeMap::new(),
        timeout_ms: 5000,
        pty: None,
        stdin: ProcessStdin::Closed,
        capture: false,
        output_cap: 0,
    };
    let registered = tokio::task::spawn_blocking({
        let key = key.clone();
        move || {
            process.agent_process(
                &ProcessRequest::RegisterAgentBash {
                    key,
                    fingerprint: spec.fingerprint(),
                    spec,
                },
                std::time::Instant::now() + std::time::Duration::from_secs(5),
                || Ok(()),
            )
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(registered, ProcessReply::Registered { key });
    let name = format!("{:032x}.output", operation.as_u128());
    let capture = std::fs::read_dir(scratch.path())
        .unwrap()
        .map(|entry| entry.unwrap().path().join(&name))
        .find(|path| path.is_file())
        .expect("physical child-owned capture");

    let host = fixture.host.clone();
    let drain = tokio::spawn(async move { host.drain().await });
    bounded(revoke).await.unwrap();
    assert!(
        !capture.exists(),
        "the physical Client removed the child capture before replying"
    );
    let cleanup = tokio::spawn(reaped.after_child_reaped());
    bounded(endpoint.settlement_joined.notified()).await;
    assert!(!cleanup.is_finished());
    release_revoke.send(()).unwrap();
    assert_eq!(bounded(drain).await.unwrap(), [] as [String; 0]);
    bounded(cleanup).await.unwrap().unwrap();
    assert!(bounded(child.wait()).await.unwrap().success());
    bounded(data).await.unwrap();
    bounded(control).await.unwrap();
    assert!(!endpoint.workspace().join("child-unpublished").exists());
    assert!(!endpoint.workspace().join("child-started").exists());
}

#[derive(Debug, PartialEq, Eq)]
enum SettlementExchange {
    Query,
    ReleaseOutput,
}

fn with_unproven_process_snapshot(
    (mut client_data, mut server_data): (UnixStream, UnixStream),
) -> tokio::task::JoinHandle<Vec<SettlementExchange>> {
    // The Client tests inject the supervisor wait failure itself. This proxy
    // injects the resulting typed snapshot at the Server's real wire boundary.
    tokio::spawn(async move {
        let mut selected = None;
        let mut trace = Vec::new();
        while let Ok(request) = receive::<ClientRequest>(&mut server_data).await {
            if let ClientRequest::Process(process) = &request {
                if matches!(
                    process,
                    ProcessRequest::Register { .. } | ProcessRequest::RegisterAgentBash { .. }
                ) {
                    selected.get_or_insert_with(|| process.key().clone());
                }
                if selected.as_ref() == Some(process.key()) {
                    match process {
                        ProcessRequest::Query { .. } => trace.push(SettlementExchange::Query),
                        ProcessRequest::ReleaseOutput { .. } => {
                            trace.push(SettlementExchange::ReleaseOutput);
                        }
                        _ => {}
                    }
                }
            }
            send(&mut client_data, &request).await.unwrap();
            let mut reply = receive::<ClientReply>(&mut client_data).await.unwrap();
            if let ClientReply::Process(ProcessReply::State {
                key,
                state: OperationState::Settled(snapshot),
            }) = &mut reply
                && selected.as_ref() == Some(key)
            {
                assert!(snapshot.physical_settlement_proven);
                assert_eq!(snapshot.exit_code, Some(0));
                snapshot.disposition = SettledDisposition::Unknown;
                snapshot.exit_code = None;
                snapshot.physical_settlement_proven = false;
            }
            send(&mut server_data, &reply).await.unwrap();
        }
        let _ = client_data.shutdown().await;
        trace
    })
}

#[allow(clippy::too_many_lines)] // Both cleanup orders use the same real Client and relay fixture.
async fn unproven_child_settlement(journal_released: bool) {
    use crate::local_runtime::session_runtime_manager::ChildExecutionRelay;
    use crate::runtime::subagent::ResolvedSubagentTool;
    use crate::tools::types::ToolInvocationPolicy;
    use rustx_execution_protocol::process::{
        OperationId, ProcessProgram, ProcessSpec, ProcessStdin,
    };
    use std::time::{Duration, Instant};

    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data,
        control: (mut client_control, mut server_control),
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let data = with_unproven_process_snapshot(data);
    let control = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut client_control, &mut server_control).await;
    });

    let session = &fixture.sessions[0];
    let conversation = &session.active_conversation_id;
    let definition =
        crate::tools::native::subagent_child_definition("bash", ToolInvocationPolicy::default())
            .unwrap();
    let relay = ChildExecutionRelay::create(
        &fixture.manager.execution_scope(&session.id),
        endpoint.workspace(),
        conversation,
        &[ResolvedSubagentTool::Builtin {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition,
        }],
        None,
    )
    .unwrap();
    let (parent, child_startup) = UnixStream::pair().unwrap();
    relay.transfer(&parent).await.unwrap();
    let scope = relay
        .grant()
        .open(&session.id, endpoint.workspace(), &child_startup)
        .await
        .unwrap();
    let owner = relay.start().unwrap();
    let process = scope.endpoint().unwrap();
    let key = process.process_key(OperationId::new()).unwrap();
    let spec = ProcessSpec {
        cwd: std::path::PathBuf::new(),
        program: ProcessProgram::Shell {
            command: "printf settled; exit 0".into(),
            conversation: conversation.as_str().into(),
            live_output: false,
        },
        env: std::collections::BTreeMap::new(),
        timeout_ms: 5_000,
        pty: None,
        stdin: ProcessStdin::Closed,
        capture: false,
        output_cap: 0,
    };
    let state = bounded(tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let exchange = |request| {
            process
                .agent_process(&request, deadline, || Ok(()))
                .unwrap()
        };
        assert_eq!(
            exchange(ProcessRequest::RegisterAgentBash {
                key: key.clone(),
                fingerprint: spec.fingerprint(),
                spec,
            }),
            ProcessReply::Registered { key: key.clone() },
        );
        let mut reply = exchange(ProcessRequest::Start { key: key.clone() });
        loop {
            if let ProcessReply::State {
                state: OperationState::Settled(snapshot),
                ..
            } = reply
            {
                assert_eq!(snapshot.disposition, SettledDisposition::Unknown);
                assert!(!snapshot.physical_settlement_proven);
                if journal_released {
                    assert_eq!(
                        exchange(ProcessRequest::ReleaseOutput {
                            key: key.clone(),
                            final_sequence: snapshot.final_sequence,
                        }),
                        ProcessReply::OutputReleased { key: key.clone() },
                    );
                }
                break;
            }
            assert!(matches!(
                reply,
                ProcessReply::State {
                    state: OperationState::Starting
                        | OperationState::Running
                        | OperationState::Exited { .. },
                    ..
                }
            ));
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
            reply = exchange(ProcessRequest::Query { key: key.clone() });
        }
    }))
    .await;
    state.unwrap();

    assert!(
        !scope.delegated_work().unwrap().seal_unused(),
        "dispatch crossed the child's no-work boundary"
    );
    bounded(crate::runtime::subagent::process::assert_unproven_relay_retains_child_root(owner))
        .await;
    assert!(
        endpoint.control().is_none(),
        "unconfirmed Open binding retires"
    );
    let failures = bounded(fixture.host.drain()).await;
    assert_eq!(failures.len(), 1, "{failures:?}");
    let trace = bounded(data).await.unwrap();
    let releases: Vec<_> = trace
        .iter()
        .enumerate()
        .filter_map(|(index, step)| (*step == SettlementExchange::ReleaseOutput).then_some(index))
        .collect();
    let [released_at] = releases[..] else {
        panic!("expected exactly one acknowledged journal release, got {trace:?}");
    };
    if journal_released {
        assert_eq!(trace[released_at + 1..], [SettlementExchange::Query]);
    } else {
        assert_eq!(trace.last(), Some(&SettlementExchange::ReleaseOutput));
        assert_eq!(trace.get(released_at - 1), Some(&SettlementExchange::Query));
    }
    bounded(child.wait()).await.unwrap();
    bounded(control).await.unwrap();
}

pub(crate) async fn child_cleanup_requires_physical_settlement_before_and_after_journal_release() {
    unproven_child_settlement(false).await;
    unproven_child_settlement(true).await;
}

#[derive(Clone, Copy)]
enum WithheldDelegatedReply {
    Register,
    PrepareWrite,
    MalformedPrepare,
    CommitWrite,
    CommitUnknown,
}

impl WithheldDelegatedReply {
    fn selects(self, request: &ClientRequest) -> bool {
        matches!(
            (self, request),
            (
                Self::Register,
                ClientRequest::Process(ProcessRequest::RegisterAgentBash { .. })
            ) | (
                Self::PrepareWrite | Self::MalformedPrepare,
                ClientRequest::PrepareWrite { .. }
            ) | (
                Self::CommitWrite | Self::CommitUnknown,
                ClientRequest::CommitWrite { .. }
            )
        )
    }
}

#[allow(clippy::too_many_lines)] // The same physical Client stays parked across receipt loss and child cleanup.
async fn withhold_delegated_reply(fault: WithheldDelegatedReply) {
    use crate::local_runtime::session_runtime_manager::ChildExecutionRelay;
    use crate::runtime::subagent::ResolvedSubagentTool;
    use crate::tools::types::ToolInvocationPolicy;
    use rustx_execution_protocol::process::{
        OperationId, ProcessProgram, ProcessSpec, ProcessStdin,
    };

    let fixture = Fixture::with_session_count(None, 1).await;
    let ProxiedClient {
        data: (mut client_data, mut server_data),
        control: (mut client_control, mut server_control),
        endpoint,
        mut child,
        _scratch: scratch,
    } = proxied_client(&fixture).await;
    let (received, receipt) = oneshot::channel();
    let (inject_fault, hold_reply) = oneshot::channel();
    let (release_data, hold_data) = oneshot::channel();
    let data = tokio::spawn(async move {
        while let Ok(request) = receive::<ClientRequest>(&mut server_data).await {
            assert!(!matches!(
                request,
                ClientRequest::Process(ProcessRequest::Start { .. })
            ));
            send(&mut client_data, &request).await.unwrap();
            let reply = receive::<ClientReply>(&mut client_data).await.unwrap();
            if fault.selects(&request) {
                received.send(reply).unwrap();
                hold_reply.await.unwrap();
                let replacement = match (fault, request) {
                    (
                        WithheldDelegatedReply::CommitUnknown,
                        ClientRequest::CommitWrite { ticket },
                    ) => Some(ClientReply::CommitUnknown { ticket }),
                    (WithheldDelegatedReply::MalformedPrepare, _) => {
                        Some(ClientReply::WritePrepared {
                            ticket: 0.into(),
                            original: None,
                        })
                    }
                    _ => None,
                };
                if let Some(reply) = replacement {
                    send(&mut server_data, &reply).await.unwrap();
                }
                drop(server_data);
                // Hold the Client-facing lane across the relay's conclusion.
                hold_data.await.unwrap();
                drop(client_data);
                return;
            }
            send(&mut server_data, &reply).await.unwrap();
        }
        panic!("expected a receipt from the real Client");
    });
    let (release_control, mut hold_control) = oneshot::channel();
    let control = tokio::spawn(async move {
        let mut server_open = true;
        loop {
            let poll = tokio::select! {
                biased;
                released = &mut hold_control => {
                    released.unwrap();
                    return;
                }
                poll = receive::<ControlPoll>(&mut client_control) => poll.unwrap()
            };
            if server_open
                && send(&mut server_control, &poll).await.is_ok()
                && let Ok(turn) = receive::<ControlTurn>(&mut server_control).await
            {
                if send(&mut client_control, &turn).await.is_err() {
                    break;
                }
                continue;
            }
            // Isolate the Server-side loss. Echo idle polls on the Client
            // side so its independent deadline cannot stand in for the gate.
            assert!(poll.receipt.is_none());
            server_open = false;
            let idle = ControlTurn {
                sequence: poll.sequence,
                health: poll.health,
                request: None,
            };
            if send(&mut client_control, &idle).await.is_err() {
                break;
            }
        }
        hold_control.await.unwrap();
    });
    let session = &fixture.sessions[0];
    let conversation = &session.active_conversation_id;
    let tools = ["bash", "write"].map(|name| {
        let definition =
            crate::tools::native::subagent_child_definition(name, ToolInvocationPolicy::default())
                .unwrap();
        ResolvedSubagentTool::Builtin {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            definition,
        }
    });
    let relay = ChildExecutionRelay::create(
        &fixture.manager.execution_scope(&session.id),
        endpoint.workspace(),
        conversation,
        &tools,
        None,
    )
    .unwrap();
    let (parent, child_startup) = UnixStream::pair().unwrap();
    relay.transfer(&parent).await.unwrap();
    let scope = relay
        .grant()
        .open(&session.id, endpoint.workspace(), &child_startup)
        .await
        .unwrap();
    let cleanup = relay.start().unwrap();
    let cancellation = detached_cancellation(crate::runtime::CancellationSignal::new());
    let operation = OperationId::new();
    let workspace_file = endpoint.workspace().join("child-owned");
    let exchange = tokio::spawn({
        let scope = scope.clone();
        let conversation = conversation.as_str().to_owned();
        async move {
            if matches!(fault, WithheldDelegatedReply::Register) {
                let process = scope.endpoint().unwrap();
                let key = process.process_key(operation).unwrap();
                let spec = ProcessSpec {
                    cwd: std::path::PathBuf::new(),
                    program: ProcessProgram::Shell {
                        command: "touch unexpected-start".into(),
                        conversation,
                        live_output: true,
                    },
                    env: std::collections::BTreeMap::new(),
                    timeout_ms: 5_000,
                    pty: None,
                    stdin: ProcessStdin::Closed,
                    capture: false,
                    output_cap: 0,
                };
                tokio::task::spawn_blocking(move || {
                    process
                        .agent_process(
                            &ProcessRequest::RegisterAgentBash {
                                key,
                                fingerprint: spec.fingerprint(),
                                spec,
                            },
                            std::time::Instant::now() + std::time::Duration::from_secs(5),
                            || Ok(()),
                        )
                        .map(drop)
                })
                .await
                .unwrap()
            } else {
                let prepared = scope
                    .prepare_write(
                        std::path::Path::new("child-owned"),
                        b"published once".to_vec(),
                        &cancellation,
                    )
                    .await?;
                scope
                    .commit_prepared(&prepared, &cancellation)
                    .await
                    .map(drop)
            }
        }
    });

    let capture = match (fault, bounded(receipt).await.unwrap()) {
        (
            WithheldDelegatedReply::Register,
            ClientReply::Process(ProcessReply::Registered { key }),
        ) => {
            assert_eq!(key.operation, operation);
            let name = format!("{:032x}.output", operation.as_u128());
            Some(
                std::fs::read_dir(scratch.path())
                    .unwrap()
                    .map(|entry| entry.unwrap().path().join(&name))
                    .find(|path| path.is_file())
                    .expect(
                        "physical allocation exists while registration acknowledgement is held",
                    ),
            )
        }
        (
            WithheldDelegatedReply::PrepareWrite | WithheldDelegatedReply::MalformedPrepare,
            ClientReply::WritePrepared { ticket, .. },
        ) => {
            assert_ne!(ticket.get(), 0);
            assert!(!workspace_file.exists());
            None
        }
        (
            WithheldDelegatedReply::CommitWrite | WithheldDelegatedReply::CommitUnknown,
            ClientReply::Written { metadata, .. },
        ) => {
            assert_eq!(metadata.size.get(), 14);
            assert_eq!(std::fs::read(&workspace_file).unwrap(), b"published once");
            None
        }
        (_, reply) => panic!("unexpected receipt: {reply:?}"),
    };
    inject_fault.send(()).unwrap();
    let error = bounded(exchange).await.unwrap().unwrap_err();
    if matches!(
        fault,
        WithheldDelegatedReply::CommitWrite | WithheldDelegatedReply::CommitUnknown
    ) {
        assert!(
            error
                .downcast::<rustx_execution_server::WriteOutcomeUnknown>()
                .is_ok()
        );
    }
    assert!(!scope.delegated_work().unwrap().seal_unused());
    bounded(crate::runtime::subagent::process::assert_unproven_relay_retains_child_root(cleanup))
        .await;
    assert!(child.try_wait().unwrap().is_none());
    assert!(endpoint.control().is_none());
    assert!(!endpoint.workspace().join("unexpected-start").exists());
    if let Some(path) = &capture {
        assert!(path.is_file(), "Client cleanup is still gated");
    }
    assert_eq!(bounded(fixture.host.drain()).await.len(), 1);
    release_control.send(()).unwrap();
    release_data.send(()).unwrap();
    bounded(data).await.unwrap();
    bounded(control).await.unwrap();
    bounded(child.wait()).await.unwrap();
    if let Some(path) = capture {
        assert!(!path.exists());
    }
}

pub(crate) async fn lost_delegated_resource_receipts_keep_physical_cleanup_unconfirmed() {
    for fault in [
        WithheldDelegatedReply::Register,
        WithheldDelegatedReply::PrepareWrite,
        WithheldDelegatedReply::MalformedPrepare,
        WithheldDelegatedReply::CommitWrite,
        WithheldDelegatedReply::CommitUnknown,
    ] {
        withhold_delegated_reply(fault).await;
    }
}

pub(crate) async fn unknown_command_settlement_preserves_the_binding_and_output() {
    let fixture = Fixture::with_session_count(None, 1).await;
    grant(&fixture);
    let ProxiedClient {
        data,
        control: (mut client_control, mut server_control),
        endpoint,
        mut child,
        _scratch,
    } = proxied_client(&fixture).await;
    let data = with_unproven_process_snapshot(data);
    let control = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut client_control, &mut server_control).await;
    });
    let connection = Arc::new(AppServerConnection::new(fixture.host.clone()));
    initialize(&connection).await;
    let MethodResult::CommandExec { result } = bounded(call(
        &connection,
        1,
        Method::CommandExec(parameters(
            &fixture,
            &["/bin/sh", "-c", "printf done; exit 0"],
        )),
    ))
    .await
    else {
        panic!("expected a domain result for unconfirmed physical settlement");
    };
    assert_eq!(result.status, ExecStatus::ProcessControlFailed);
    assert_eq!(result.exit_code, None);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result.stdout_base64)
            .unwrap(),
        b"done"
    );
    assert!(endpoint.control().is_some());
    let result = bounded(call(
        &connection,
        2,
        Method::CommandExec(parameters(&fixture, &["/bin/true"])),
    ))
    .await;
    assert!(
        matches!(result, MethodResult::CommandExec { result } if result.status == ExecStatus::Completed && result.exit_code == Some(0))
    );
    connection.close();
    // Only the wire snapshot was fault-injected. The real Client can prove
    // binding-wide settlement and returns an authentic Revoke receipt.
    assert_eq!(bounded(fixture.host.drain()).await, [] as [String; 0]);
    let trace = bounded(data).await.unwrap();
    assert_eq!(trace.last(), Some(&SettlementExchange::ReleaseOutput));
    assert!(bounded(child.wait()).await.unwrap().success());
    bounded(control).await.unwrap();
}
