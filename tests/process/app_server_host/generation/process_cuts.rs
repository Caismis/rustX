//! A real Client generation loses its process lane at specific ownership cuts.

use super::*;
use rustx::app_server::command::ExecParams;
use rustx_execution_protocol::process::{OperationState, ProcessReply, ProcessRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessCut {
    Registered,
    CompanionSpawn,
    Running,
    TerminalReply,
}

impl ProcessCut {
    fn reached(self, reply: &ProcessReply) -> bool {
        matches!(
            (self, reply),
            (Self::Registered, ProcessReply::Registered { .. })
                | (
                    Self::CompanionSpawn,
                    ProcessReply::State {
                        state: OperationState::Starting,
                        ..
                    }
                )
                | (
                    Self::Running,
                    ProcessReply::State {
                        state: OperationState::Running,
                        ..
                    }
                )
                | (
                    Self::TerminalReply,
                    ProcessReply::State {
                        state: OperationState::Settled(_),
                        ..
                    }
                )
        )
    }
}

fn park_companion(binary: &Path, socket: &Path) {
    let executable = binary.with_file_name("bash-supervisor");
    let real = binary.with_file_name("bash-supervisor-real");
    std::fs::rename(&executable, &real).unwrap();
    std::fs::write(
        &executable,
        format!(
            r"#!/usr/bin/python3
import os, socket, sys
with socket.socket(socket.AF_UNIX) as witness:
    witness.connect({socket:?})
    witness.sendall(b'exec-entry')
    assert witness.recv(1) == b'G'
os.execv({real:?}, [{real:?}, *sys.argv[1:]])
",
            socket = socket.display().to_string(),
            real = real.display().to_string(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
}

async fn forward_to_cut(
    mut client: UnixStream,
    mut server: UnixStream,
    cut: ProcessCut,
    reached: oneshot::Sender<(usize, usize, ProcessReply)>,
    killed: oneshot::Receiver<Option<oneshot::Sender<ClientReply>>>,
) {
    let mut registrations = 0;
    let mut starts = 0;
    while let Ok(request) = read::<ClientRequest>(&mut server).await {
        if let ClientRequest::Process(operation) = &request {
            registrations += usize::from(matches!(operation, ProcessRequest::Register { .. }));
            starts += usize::from(matches!(operation, ProcessRequest::Start { .. }));
        }
        write(&mut client, &request).await.unwrap();
        let reply = read::<ClientReply>(&mut client).await.unwrap();
        if let ClientReply::Process(reply) = &reply
            && cut.reached(reply)
        {
            reached
                .send((registrations, starts, reply.clone()))
                .unwrap();
            if let Ok(Some(diagnostics)) = killed.await
                && let ProcessReply::State { key, .. } = reply
            {
                let request = ClientRequest::Process(ProcessRequest::OutputPage {
                    key: key.clone(),
                    cursor: rustx_execution_protocol::DecimalU64::new(0),
                    max_bytes: 4096,
                });
                write(&mut client, &request).await.unwrap();
                let response = read::<ClientReply>(&mut client).await.unwrap();
                diagnostics.send(response).ok().unwrap();
            }
            return;
        }
        write(&mut server, &reply).await.unwrap();
    }
    panic!("original Client stopped before {cut:?}");
}

async fn reject_process_replay(mut client: UnixStream, mut server: UnixStream) {
    while let Ok(request) = read::<ClientRequest>(&mut server).await {
        assert!(
            !matches!(request, ClientRequest::Process(_)),
            "replacement never receives an old process request"
        );
        write(&mut client, &request).await.unwrap();
        let reply = read::<ClientReply>(&mut client).await.unwrap();
        write(&mut server, &reply).await.unwrap();
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one real command follows each physical cut to replacement"
)]
async fn prove_process_death(cut: ProcessCut) {
    let Some(provider) = ProviderEmulator::start("app_server_host_broadcast").await else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    config(root.path(), &workspace, &provider);
    let probe = workspace.join("probe");
    std::fs::write(&probe, b"original Workspace").unwrap();
    let (session, thread, node) = create_host_session(root.path(), &workspace).await;

    let (_bins, binary) = super::super::super::app_server::isolated_server_pair();
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let witness_path = scratch.path().join("witness");
    let listener = UnixListener::bind(&witness_path).unwrap();
    stage_wrapper(&binary, &witness_path);
    let companion_socket = scratch.path().join("companion.sock");
    let companion_witness = UnixListener::bind(&companion_socket).unwrap();
    let business_witness = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    if cut == ProcessCut::CompanionSpawn {
        park_companion(&binary, &companion_socket);
    }

    let (old_sender, old_started) = oneshot::channel();
    let (new_sender, new_started) = oneshot::channel();
    let (cut_sender, cut_reached) = oneshot::channel();
    let (killed, close_old_lane) = oneshot::channel();
    let relay = tokio::spawn(async move {
        let (old, (data, server_data), (mut control, mut server_control)) =
            proxy_launch(&listener, scratch.path(), 1).await;
        old_sender.send(old).ok().unwrap();
        let old_lanes = tokio::spawn(async move {
            let _ = tokio::join!(
                forward_to_cut(data, server_data, cut, cut_sender, close_old_lane),
                tokio::io::copy_bidirectional(&mut control, &mut server_control),
            );
        });
        let (new, (data, server_data), (mut control, mut server_control)) =
            proxy_launch(&listener, scratch.path(), 2).await;
        new_sender.send(new).ok().unwrap();
        let _ = tokio::join!(
            reject_process_replay(data, server_data),
            tokio::io::copy_bidirectional(&mut control, &mut server_control),
        );
        old_lanes.await.unwrap();
    });
    let mut command = server_command(root.path(), &binary);
    command.args([
        "--local-execution-session",
        session.as_str(),
        "--local-execution-workspace",
    ]);
    command.arg(&workspace);
    let (server, address, diagnostics) = launch(command).await;
    let original = old_started.await.unwrap();
    let mut peer = Peer::connect(&address).await;

    let argv = vec![
        "/usr/bin/python3".to_owned(),
        "-c".to_owned(),
        r"
import pathlib, socket, sys
with pathlib.Path('starts').open('ab') as marker:
    marker.write(b'one start' + bytes([10]))
if sys.argv[2] == 'stay':
    with socket.create_connection(('127.0.0.1', int(sys.argv[1]))) as witness:
        witness.sendall(b'physical-start')
        witness.recv(1)
"
        .to_owned(),
        business_witness.local_addr().unwrap().port().to_string(),
        if cut == ProcessCut::Running {
            "stay"
        } else {
            "exit"
        }
        .to_owned(),
    ];
    let mut execution = Box::pin(peer.raw_request(json!({
        "jsonrpc": "2.0", "method": "command/exec",
        "params": ExecParams {
            session_id: session.clone(), command: argv,
            cwd: workspace.display().to_string(), env: std::collections::BTreeMap::new(), process_id: None,
            tty: false, stream_stdin: false, stream_stdout_stderr: false,
            output_bytes_cap: None, timeout_ms: None, size: None,
        },
    })));
    let marker = workspace.join("starts");
    let (registrations, starts, receipt) = tokio::select! {
        reached = cut_reached => reached.unwrap(),
        result = &mut execution => panic!("command finished before {cut:?}: {result:?}"),
    };
    assert_eq!(registrations, 1);
    assert_eq!(starts, usize::from(cut != ProcessCut::Registered));
    let mut companion = None;
    let mut business = None;
    if cut == ProcessCut::CompanionSpawn {
        let (mut child, _) = companion_witness.accept().await.unwrap();
        let mut evidence = [0; 10];
        child.read_exact(&mut evidence).await.unwrap();
        assert_eq!(&evidence, b"exec-entry");
        companion = Some(child);
    } else if cut == ProcessCut::Running {
        let accepted =
            tokio::time::timeout(Duration::from_secs(5), business_witness.accept()).await;
        let Ok(accepted) = accepted else {
            let (report, diagnostic) = oneshot::channel();
            killed.send(Some(report)).unwrap();
            let reply = diagnostic.await.unwrap();
            let bytes = match reply {
                ClientReply::Process(ProcessReply::Output { delta_base64, .. }) => {
                    String::from_utf8_lossy(&STANDARD.decode(delta_base64).unwrap()).into_owned()
                }
                other => format!("{other:?}"),
            };
            panic!(
                "business never reached its TCP gate: {bytes}; starts: {:?}",
                std::fs::read(&marker)
            );
        };
        let (mut child, _) = accepted.unwrap();
        let mut evidence = [0; 14];
        child.read_exact(&mut evidence).await.unwrap();
        assert_eq!(&evidence, b"physical-start");
        business = Some(child);
    }
    if let ProcessReply::State {
        state: OperationState::Settled(snapshot),
        ..
    } = receipt
    {
        assert_eq!(cut, ProcessCut::TerminalReply);
        assert!(snapshot.physical_settlement_proven);
        assert_eq!(snapshot.exit_code, Some(0));
    }
    let crossed = matches!(cut, ProcessCut::Running | ProcessCut::TerminalReply);
    if crossed {
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            b"one start
"
        );
    } else {
        assert!(!marker.exists());
    }

    kill(original.pid, Signal::SIGKILL).unwrap();
    if let Some(companion) = &mut companion {
        assert_eq!(companion.read(&mut [0]).await.unwrap(), 0);
    }
    if let Some(business) = &mut business {
        assert_eq!(business.read(&mut [0]).await.unwrap(), 0);
    }
    killed.send(None).unwrap();
    let Response::Failure(failure) = execution.await else {
        panic!("a lost Client cannot report command success at {cut:?}");
    };
    if cut != ProcessCut::Registered {
        assert_eq!(failure.error.data, Some(ErrorData::OutcomeUnknown));
    }
    let replacement = new_started.await.unwrap();
    assert_ne!(replacement.pid, original.pid);
    assert_eq!(replacement.admission.session, original.admission.session);
    assert_ne!(replacement.admission.instance, original.admission.instance);
    assert!(!original.scratch.exists());

    assert_replacement_reads_workspace(&mut peer, &session, &probe).await;
    if crossed {
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            b"one start
"
        );
    } else {
        assert!(!marker.exists());
    }

    drop(peer);
    shutdown(server, diagnostics).await;
    relay.await.unwrap();
    assert_eq!(provider.requests().await.len(), 0);
    let sessions = SessionController::open(&root.path().join("runtime")).unwrap();
    let access = sessions
        .acquire_session(&session, Some(&node))
        .await
        .unwrap();
    let store = SqliteConversationStore::open_existing(thread, &access.database_path).unwrap();
    assert_eq!(
        store.load_canonical().unwrap().len(),
        0,
        "UI argv has no Agent settlement"
    );
    // Command/exec has no model Attempt. No scenario response should be consumed.
    provider.abort().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_death_at_wire_and_companion_boundaries_never_replays_start() {
    for cut in [
        ProcessCut::Registered,
        ProcessCut::CompanionSpawn,
        ProcessCut::Running,
        ProcessCut::TerminalReply,
    ] {
        tokio::time::timeout(Duration::from_secs(30), prove_process_death(cut))
            .await
            .unwrap_or_else(|_| panic!("real Client did not settle at {cut:?}"));
    }
}
