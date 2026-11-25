//! Real installed Server/Client with separate socket owners and parked execution lanes.

use super::*;
use rustx::app_server::command::ExecParams;
use rustx_execution_protocol::{
    ControlPoll, ControlReply, ControlTurn, ProcessCancelDisposition,
    process::{OperationState, ProcessReply, ProcessRequest, SettledDisposition, SettledSnapshot},
    transport::MAX_CONTROL_COMMANDS,
};

#[derive(Default)]
struct ProcessTrace {
    registered: usize,
    started: usize,
    settled: Option<SettledSnapshot>,
}

async fn data_lane(
    mut client: UnixStream,
    mut server: UnixStream,
    parked: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
    released: oneshot::Sender<(
        rustx_execution_protocol::process::ProcessOperationKey,
        ProcessTrace,
    )>,
) {
    let mut gate = Some((parked, resume));
    let mut released = Some(released);
    let mut trace = ProcessTrace::default();
    while let Ok(request) = read::<ClientRequest>(&mut server).await {
        write(&mut client, &request).await.unwrap();
        let reply = read::<ClientReply>(&mut client).await.unwrap();
        if matches!(&request, ClientRequest::GetMetadata { relative_path } if relative_path.as_os_str() == "block-data")
            && let Some((parked, resume)) = gate.take()
        {
            assert!(matches!(reply, ClientReply::Metadata { .. }));
            parked.send(()).unwrap();
            resume.await.unwrap();
        }
        match (&request, &reply) {
            (ClientRequest::Process(ProcessRequest::Register { .. }), _) => trace.registered += 1,
            (ClientRequest::Process(ProcessRequest::Start { .. }), _) => trace.started += 1,
            (
                _,
                ClientReply::Process(ProcessReply::State {
                    state: OperationState::Settled(snapshot),
                    ..
                }),
            ) => {
                trace.settled = Some(*snapshot);
            }
            (
                ClientRequest::Process(ProcessRequest::ReleaseOutput {
                    key,
                    final_sequence,
                }),
                ClientReply::Process(ProcessReply::OutputReleased { key: echoed }),
            ) => {
                assert_eq!(key, echoed);
                assert_eq!(
                    trace
                        .settled
                        .expect("physical settlement precedes ReleaseOutput")
                        .final_sequence,
                    *final_sequence
                );
                released
                    .take()
                    .expect("one output release")
                    .send((key.clone(), std::mem::take(&mut trace)))
                    .ok()
                    .unwrap();
            }
            _ => {}
        }
        if write(&mut server, &reply).await.is_err() {
            break;
        }
    }
}

async fn control_lane(
    mut client: UnixStream,
    mut server: UnixStream,
    parked: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
    receipts: tokio::sync::mpsc::UnboundedSender<ControlReply>,
) {
    let mut gate = Some((parked, resume));
    while let Ok(poll) = read::<ControlPoll>(&mut client).await {
        if matches!(poll.receipt, Some(ControlReply::WatchStopped { .. }))
            && let Some((parked, resume)) = gate.take()
        {
            parked.send(()).unwrap();
            resume.await.unwrap();
        }
        if let Some(receipt) = &poll.receipt {
            let _ = receipts.send(receipt.clone());
        }
        if write(&mut server, &poll).await.is_err() {
            break;
        }
        let Ok(turn) = read::<ControlTurn>(&mut server).await else {
            break;
        };
        if write(&mut client, &turn).await.is_err() {
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_ui_detach_cancels_the_client_with_both_lanes_occupied() {
    bounded(async {
        let Some(provider) = ProviderEmulator::start("app_server_host_broadcast").await else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        config(root.path(), &workspace, &provider);
        let (session, _, _) = create_host_session(root.path(), &workspace).await;
        let (_bins, executable) = super::super::super::app_server::isolated_server_pair();
        let scratch = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let witness_path = scratch.path().join("witness.sock");
        let witness = UnixListener::bind(&witness_path).unwrap();
        stage_wrapper(&executable, &witness_path);

        let (data_parked, data_blocked) = oneshot::channel();
        let (resume_data, resume_data_gate) = oneshot::channel();
        let (output_released, mut output) = oneshot::channel();
        let (control_parked, control_blocked) = oneshot::channel();
        let (resume_control, resume_control_gate) = oneshot::channel();
        let (receipt_tx, mut receipts) = tokio::sync::mpsc::unbounded_channel();
        let relay = tokio::spawn(async move {
            let (_, (data, server_data), (control, server_control)) =
                proxy_launch(&witness, scratch.path(), 1).await;
            tokio::join!(
                data_lane(
                    data,
                    server_data,
                    data_parked,
                    resume_data_gate,
                    output_released
                ),
                control_lane(
                    control,
                    server_control,
                    control_parked,
                    resume_control_gate,
                    receipt_tx
                ),
            );
        });
        let mut command = server_command(root.path(), &executable);
        command
            .args([
                "--local-execution-session",
                session.as_str(),
                "--local-execution-workspace",
            ])
            .arg(&workspace);
        let (server, address, diagnostics) = launch(command).await;

        let mut watchers = Vec::new();
        for id in 0..MAX_CONTROL_COMMANDS {
            let mut watcher = Peer::connect(&address).await;
            let name = format!("watched-{id}");
            let path = workspace.join(&name);
            std::fs::write(&path, b"unchanged").unwrap();
            assert!(matches!(
                watcher
                    .rpc(Method::FsWatch {
                        session_id: session.clone(),
                        watch_id: name,
                        path: path.display().to_string(),
                    })
                    .await,
                MethodResult::EnvironmentAccepted {}
            ));
            watchers.push(watcher);
        }
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut owner = Peer::connect(&address).await;
        owner
            .reply(
                json!({"jsonrpc": "2.0", "id": 900, "method": "command/exec", "params": ExecParams {
                    session_id: session.clone(),
                    command: vec!["/bin/bash".into(), "-c".into(),
                        format!("exec 3<>/dev/tcp/127.0.0.1/{port}; printf READY; read -r -u 3")],
                    cwd: workspace.display().to_string(), env: std::collections::BTreeMap::new(),
                    process_id: Some("installed-detach".into()), tty: false, stream_stdin: false,
                    stream_stdout_stderr: true, output_bytes_cap: None, timeout_ms: None, size: None
                }}),
            )
            .await;
        let (mut process, _) = listener.accept().await.unwrap();
        let mut visible = Vec::new();
        while visible.len() < b"READY".len() {
            let ServerOutput::Notification(event) = owner.receive().await else {
                panic!("command settled before losing its UI owner");
            };
            if let NotificationMethod::CommandOutputDelta(output) = event.notification {
                assert_eq!(output.process_id, "installed-detach");
                visible.extend(STANDARD.decode(output.delta_base64).unwrap());
            }
        }
        assert_eq!(visible, b"READY");

        let path = workspace.join("block-data");
        std::fs::write(&path, b"unchanged").unwrap();
        let mut reader = Peer::connect(&address).await;
        let mut query = Box::pin(async {
            loop {
                match reader
                    .response(Method::FsGetMetadata {
                        session_id: session.clone(),
                        path: path.display().to_string(),
                    })
                    .await
                {
                    Response::Failure(failure)
                        if failure.error.data == Some(ErrorData::EnvironmentCapacityOrConflict) =>
                    {
                        tokio::task::yield_now().await;
                    }
                    other => break other,
                }
            }
        });
        tokio::select! {
            blocked = data_blocked => blocked.unwrap(),
            premature = &mut query => panic!("query was not held: {premature:?}"),
        }
        for (id, watcher) in watchers.iter_mut().enumerate() {
            watcher
                .reply(
                    json!({"jsonrpc": "2.0", "id": 910 + id, "method": "fs/unwatch",
                "params": {"watchId": format!("watched-{id}")}}),
                )
                .await;
        }
        control_blocked.await.unwrap();
        let mut unrelated = Peer::connect(&address).await;
        assert!(matches!(
            unrelated
                .rpc(Method::SettingsRead {
                    session_id: session.clone()
                })
                .await,
            MethodResult::Settings { .. }
        ));
        drop(owner);
        resume_control.send(()).unwrap();

        let mut stopped = std::collections::BTreeSet::new();
        let mut cancelled = None;
        while stopped.len() < MAX_CONTROL_COMMANDS || cancelled.is_none() {
            match receipts.recv().await.unwrap() {
                ControlReply::WatchStopped { id, settled } => {
                    assert!(settled);
                    assert!(stopped.insert(id));
                }
                ControlReply::ProcessCancelled { key, disposition } => {
                    assert_eq!(disposition, ProcessCancelDisposition::CancellationRequested);
                    assert!(cancelled.replace(key).is_none());
                }
                other => panic!("unexpected control receipt {other:?}"),
            }
        }
        assert_eq!(
            process.read(&mut [0; 1]).await.unwrap(),
            0,
            "physical command exited before data resumes"
        );
        assert!(matches!(
            output.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        resume_data.send(()).unwrap();
        assert!(matches!(query.await, Response::Success(_)));
        let (key, trace) = output.await.unwrap();
        assert_eq!(Some(key), cancelled);
        assert_eq!((trace.registered, trace.started), (1, 1));
        let settlement = trace.settled.unwrap();
        assert_eq!(settlement.disposition, SettledDisposition::Cancelled);
        assert!(settlement.physical_settlement_proven);
        assert_eq!(settlement.stdout_total.get(), 5);

        for (index, mut watcher) in watchers.into_iter().enumerate() {
            loop {
                match watcher.receive().await {
                    ServerOutput::Response(Response::Success(success)) => {
                        assert!(matches!(
                            success.result,
                            MethodResult::EnvironmentAccepted {}
                        ));
                        assert_eq!(
                            success.id,
                            RequestId::Integer(910 + i64::try_from(index).unwrap())
                        );
                        break;
                    }
                    ServerOutput::Notification(_) => {}
                    other => panic!("watch cleanup did not complete: {other:?}"),
                }
            }
        }
        drop(unrelated);
        terminate(&server);
        let mut server = server;
        assert!(server.wait().await.unwrap().success());
        diagnostics.await.unwrap();
        relay.await.unwrap();
        assert_eq!(receipts.recv().await, Some(ControlReply::Revoked));
        assert_eq!(receipts.recv().await, None);
        assert_eq!(provider.requests().await.len(), 0);
    })
    .await;
}
