//! Hosted output custody across real Client death at the storage and canonical cuts.

use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use rustx::app_server::host_upload::{MAX_CHUNK_BYTES, canonical_digest};
use rustx::tools::{ManagedOutputContinuation, limits::FOREGROUND_TOOL_RESULT_PREVIEW_BYTES};
use rustx_execution_protocol::{
    Admission, Bootstrap, BootstrapEndpoint, ClientReply, ClientRequest, ControlHelloReply,
    DataHelloReply, PeerHello,
    save::{SaveReply, SaveRequest},
    transport::{MAX_RECORD_BYTES, read_record, write_record},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    net::{UnixListener, UnixStream},
    sync::{oneshot, watch},
};

struct Launch {
    admission: Admission,
    pid: Pid,
    scratch: PathBuf,
    proxy: UnixListener,
    server_socket: PathBuf,
}

mod process_cuts;

const START: &str = "start-of-host-output";
const END: &str = "end-of-host-output";
const FILL: usize = 240_000;
const LENGTH: u64 = (START.len() + FILL + END.len()) as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrashCut {
    Allocated,
    LastWrite,
    Sealed,
    PublishReceipt,
    Canonical,
}

impl CrashCut {
    fn physical_length(self, request: &ClientRequest, reply: &SaveReply) -> Option<u64> {
        match (self, request, reply) {
            (
                Self::Allocated,
                ClientRequest::Save(SaveRequest::Begin { .. }),
                SaveReply::Begun { .. },
            ) => Some(0),
            (
                Self::LastWrite,
                ClientRequest::Save(SaveRequest::Chunk { .. }),
                SaveReply::Chunked { next_offset, .. },
            ) if next_offset.get() == LENGTH => Some(LENGTH),
            (
                Self::Sealed,
                ClientRequest::Save(SaveRequest::Seal { .. }),
                SaveReply::Sealed { length, .. },
            )
            | (
                Self::PublishReceipt | Self::Canonical,
                ClientRequest::Save(SaveRequest::Publish { .. }),
                SaveReply::Published { length, .. },
            ) => Some(length.get()),
            _ => None,
        }
    }
}

struct ObservedSave {
    receipt: SaveReply,
    length: u64,
    late_delivery: Option<LateDelivery>,
}

struct LateDelivery {
    resume: oneshot::Sender<()>,
    observed: oneshot::Receiver<io::Result<()>>,
}

async fn read<T: serde::de::DeserializeOwned>(lane: &mut UnixStream) -> io::Result<T> {
    let length = lane.read_u32().await? as usize;
    assert!((1..=MAX_RECORD_BYTES).contains(&length));
    let mut bytes = vec![0; length];
    lane.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

async fn write(lane: &mut UnixStream, record: &impl serde::Serialize) -> io::Result<()> {
    let mut bytes = Vec::new();
    write_record(&mut bytes, record)?;
    lane.write_all(&bytes).await
}

async fn pair(
    proxy: &UnixListener,
    path: &Path,
) -> (
    (UnixStream, UnixStream),
    (UnixStream, UnixStream),
    Admission,
) {
    let (mut data, _) = proxy.accept().await.unwrap();
    let mut server_data = UnixStream::connect(path).await.unwrap();
    let hello = read::<PeerHello>(&mut data).await.unwrap();
    assert!(matches!(hello, PeerHello::Data(_)));
    write(&mut server_data, &hello).await.unwrap();
    let reply = read::<DataHelloReply>(&mut server_data).await.unwrap();
    assert!(reply.is_ok());
    write(&mut data, &reply).await.unwrap();

    let (mut control, _) = proxy.accept().await.unwrap();
    let mut server_control = UnixStream::connect(path).await.unwrap();
    let hello = read::<PeerHello>(&mut control).await.unwrap();
    assert!(matches!(hello, PeerHello::Control(_)));
    write(&mut server_control, &hello).await.unwrap();
    let reply = read::<ControlHelloReply>(&mut server_control)
        .await
        .unwrap();
    write(&mut control, &reply).await.unwrap();
    (
        (data, server_data),
        (control, server_control),
        reply.unwrap(),
    )
}

fn stage_wrapper(binary: &Path, witness: &Path) {
    let wrapper = binary.with_file_name("rustx-execution-client");
    let client = binary.with_file_name("client-real");
    std::fs::rename(&wrapper, &client).unwrap();
    std::fs::write(
        &wrapper,
        format!(
            r"#!/usr/bin/python3
import json, os, socket, sys
assert os.getpid() == 1
arg = sys.argv.index('--binding-file') + 1
with socket.socket(socket.AF_UNIX) as peer:
    peer.connect({witness:?})
    peer.sendall((json.dumps([sys.argv[arg], os.environ['TMPDIR']]) + '\n').encode())
    sys.argv[arg] = json.loads(peer.makefile().readline())
os.execv({client:?}, [{client:?}, *sys.argv[1:]])
",
            witness = witness.display().to_string(),
            client = client.display().to_string(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
}

async fn proxy_launch(
    witness: &UnixListener,
    scratch: &Path,
    generation: u64,
) -> (Launch, (UnixStream, UnixStream), (UnixStream, UnixStream)) {
    let (witness, _) = witness.accept().await.unwrap();
    let pid = Pid::from_raw(witness.peer_cred().unwrap().pid().unwrap());
    let mut witness = BufReader::new(witness);
    let mut line = String::new();
    witness.read_line(&mut line).await.unwrap();
    let (bundle, client_scratch): (PathBuf, PathBuf) = serde_json::from_str(&line).unwrap();
    let mut bootstrap: Bootstrap = read_record(&mut std::fs::File::open(bundle).unwrap()).unwrap();
    assert_eq!(bootstrap.generation.get(), generation);
    let proxy_path = scratch.join(format!("execution-{generation}.sock"));
    let proxy = UnixListener::bind(&proxy_path).unwrap();
    let BootstrapEndpoint::Unix { socket_path } = std::mem::replace(
        &mut bootstrap.endpoint,
        BootstrapEndpoint::Unix {
            socket_path: proxy_path,
        },
    ) else {
        panic!("local Linux Client uses Unix lanes");
    };
    let bundle = scratch.join(format!("bootstrap-{generation}"));
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&bundle)
        .unwrap();
    write_record(&mut output, &bootstrap).unwrap();
    drop(output);
    let mut witness = witness.into_inner();
    witness
        .write_all(serde_json::to_string(&bundle).unwrap().as_bytes())
        .await
        .unwrap();
    witness.write_all(b"\n").await.unwrap();
    let (data, control, admission) = pair(&proxy, &socket_path).await;
    assert_eq!(admission.generation.get(), generation);
    assert_eq!(admission.instance, bootstrap.instance);
    (
        Launch {
            admission,
            pid,
            scratch: client_scratch,
            proxy,
            server_socket: socket_path,
        },
        data,
        control,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleted_session_recovers_only_for_cleanup_in_the_original_client() {
    use rustx::app_server::session_deletion::SessionDeletionResult;
    use rustx_execution_protocol::{ControlPoll, ControlReply, ControlRequest, ControlTurn};

    tokio::time::timeout(Duration::from_secs(30), async {
        let Some(provider) = ProviderEmulator::start("app_server_host_broadcast").await else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        config(root.path(), &workspace, &provider);
        let probe = workspace.join("probe");
        std::fs::write(&probe, b"original Workspace").unwrap();
        let (session, _, _) = create_host_session(root.path(), &workspace).await;
        let (_bins, binary) = super::super::app_server::isolated_server_pair();
        let scratch = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let witness_path = scratch.path().join("witness");
        let witness = UnixListener::bind(&witness_path).unwrap();
        stage_wrapper(&binary, &witness_path);
        let mut command = server_command(root.path(), &binary);
        command
            .args([
                "--local-execution-session",
                session.as_str(),
                "--local-execution-workspace",
            ])
            .arg(&workspace);
        let (
            (server, endpoint, diagnostics),
            (old, (client_data, server_data), (mut control, mut server_control)),
        ) = tokio::join!(launch(command), proxy_launch(&witness, scratch.path(), 1));
        let mut peer = Peer::connect(&endpoint).await;
        let data = tokio::spawn(reject_save_replay(client_data, server_data));
        let control = tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut control, &mut server_control).await;
        });
        let read_probe = || Method::FsReadFile {
            session_id: session.clone(),
            path: probe.display().to_string(),
            offset: 0,
            max_bytes: 32,
            expected_revision: None,
        };
        assert!(matches!(
            peer.response(read_probe()).await,
            Response::Success(_)
        ));
        control.abort();
        assert!(control.await.unwrap_err().is_cancelled());
        data.await.unwrap();

        let MethodResult::Deletion {
            result: SessionDeletionResult::Preview { preview },
        } = peer
            .rpc(Method::SessionDeletePreview {
                session_id: session.clone(),
            })
            .await
        else {
            panic!("deletion preview")
        };
        let deleted = peer
            .rpc(Method::SessionDelete {
                session_id: session.clone(),
                expected_target_revision: preview.target_revision,
            })
            .await;
        assert!(matches!(deleted, MethodResult::Deletion {
            result: SessionDeletionResult::CommittedCleanupPending { session_id },
        } if session_id == session));
        let check_catalog = || Method::SessionList {
            query: None,
            offset: 0,
            limit: 32,
        };
        let MethodResult::Sessions { sessions, .. } = peer.rpc(check_catalog()).await else {
            panic!("Session catalog")
        };
        assert!(sessions.iter().all(|record| record.id != session));
        let ((mut client_data, mut server_data), (mut control, mut server_control), recovered) =
            pair(&old.proxy, &old.server_socket).await;
        assert_eq!(recovered, old.admission);
        let data = tokio::spawn(async move {
            assert_eq!(
                server_data.read(&mut [0; 1]).await.unwrap(),
                0,
                "cleanup never dispatches to the data lane"
            );
            let _ = client_data.shutdown().await;
        });
        let mut revoked = 0;
        loop {
            let poll = read::<ControlPoll>(&mut control).await.unwrap();
            write(&mut server_control, &poll).await.unwrap();
            let turn = read::<ControlTurn>(&mut server_control).await.unwrap();
            if let Some(request) = &turn.request {
                assert!(matches!(request, ControlRequest::Revoke));
                revoked += 1;
            }
            write(&mut control, &turn).await.unwrap();
            if poll.receipt == Some(ControlReply::Revoked) {
                assert!(turn.request.is_none());
                break;
            }
        }
        assert_eq!(revoked, 1);
        data.await.unwrap();
        let recovered = peer
            .rpc(Method::SessionRecoverDeletion {
                session_id: session.clone(),
            })
            .await;
        assert!(matches!(recovered, MethodResult::Deletion {
            result: SessionDeletionResult::Deleted { session_id },
        } if session_id == session));
        let MethodResult::Sessions { sessions, .. } = peer.rpc(check_catalog()).await else {
            panic!("Session catalog after cleanup")
        };
        assert!(sessions.iter().all(|record| record.id != session));
        assert!(matches!(
            peer.response(read_probe()).await,
            Response::Failure(_)
        ));
        assert_eq!(std::fs::read(&probe).unwrap(), b"original Workspace");
        assert_eq!(provider.requests().await.len(), 0);
        assert!(matches!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(witness.poll_accept(cx))).await,
            std::task::Poll::Pending
        ));
        drop(peer);
        shutdown(server, diagnostics).await;
    })
    .await
    .expect("installed Client cleanup recovery");
}

async fn observe_save_cut(
    mut client: UnixStream,
    mut server: UnixStream,
    offsets: watch::Sender<u64>,
    cut: CrashCut,
    observed: oneshot::Sender<ObservedSave>,
) {
    let mut observed = Some(observed);
    while let Ok(request) = read::<ClientRequest>(&mut server).await {
        if write(&mut client, &request).await.is_err() {
            break;
        }
        let Ok(reply) = read::<ClientReply>(&mut client).await else {
            break;
        };
        let ClientReply::Save(receipt) = &reply else {
            write(&mut server, &reply).await.unwrap();
            continue;
        };
        if let SaveReply::Chunked {
            next_offset,
            duplicate,
            ..
        } = receipt
        {
            assert!(!duplicate);
            offsets.send_replace(next_offset.get());
        }
        if let Some(length) = cut.physical_length(&request, receipt) {
            let observed = observed.take().expect("one physical cut");
            if cut == CrashCut::Canonical {
                write(&mut server, &reply).await.unwrap();
                observed
                    .send(ObservedSave {
                        receipt: receipt.clone(),
                        length,
                        late_delivery: None,
                    })
                    .ok()
                    .unwrap();
                continue;
            }
            let (resume, release) = oneshot::channel();
            let (late, late_receipt) = oneshot::channel();
            observed
                .send(ObservedSave {
                    receipt: receipt.clone(),
                    length,
                    late_delivery: Some(LateDelivery {
                        resume,
                        observed: late_receipt,
                    }),
                })
                .ok()
                .unwrap();
            release.await.unwrap();
            late.send(write(&mut server, &reply).await).ok().unwrap();
            let mut trailing = [0];
            assert!(matches!(server.read(&mut trailing).await, Ok(0) | Err(_)));
            return;
        }
        write(&mut server, &reply).await.unwrap();
    }
    assert!(observed.is_none(), "the original Client reached {cut:?}");
}

async fn reject_save_replay(mut client: UnixStream, mut server: UnixStream) {
    while let Ok(request) = read::<ClientRequest>(&mut server).await {
        assert!(
            !matches!(request, ClientRequest::Save(_)),
            "replacement must never replay Save"
        );
        write(&mut client, &request).await.unwrap();
        let reply = read::<ClientReply>(&mut client).await.unwrap();
        write(&mut server, &reply).await.unwrap();
    }
}

fn config(root: &Path, workspace: &Path, provider: &ProviderEmulator) {
    std::fs::create_dir(workspace).unwrap();
    std::fs::create_dir(root.join("home")).unwrap();
    std::fs::write(root.join("token"), "trusted-host-test").unwrap();
    std::fs::write(root.join("rustx.toml"), toml::to_string_pretty(&json!({
        "approval_mode": "full_access",
        "agent": {"model": {"model": "emulator/chat-model"}, "tools": {"builtin": []}},
        "providers": {"emulator": {"protocol": "openai_chat_completions", "base_url": provider.openai_base_url(), "api_key": "$HOST_TEST_KEY"}},
        "models": {"emulator/chat-model": {"provider": "emulator", "id": "chat-model", "context_window": 128_000, "max_output_tokens": 1024, "capabilities": {"input_modalities": ["text"], "output_modalities": ["text"]}, "compat": {"chat_reasoning_replay": "omit"}}},
        "app_server": {"environment_roots": [workspace]}
    })).unwrap()).unwrap();
}

async fn shutdown(mut process: Child, diagnostics: tokio::task::JoinHandle<()>) {
    kill(
        Pid::from_raw(i32::try_from(process.id().unwrap()).unwrap()),
        Signal::SIGTERM,
    )
    .unwrap();
    assert!(process.wait().await.unwrap().success());
    diagnostics.await.unwrap();
}

async fn upload(peer: &mut Peer, verb: &str, params: Value) {
    let response = peer
        .raw_request(json!({
            "jsonrpc": "2.0", "method": format!("item/tool/result/{verb}"), "params": params
        }))
        .await;
    assert!(matches!(response, Response::Success(success)
        if matches!(success.result, MethodResult::HostUploadAccepted { duplicate: false })));
}

async fn upload_result(
    peer: &mut Peer,
    request: &ServerRequest,
    mut progress: Option<watch::Receiver<u64>>,
) -> usize {
    let ServerRequestMethod::ToolCall(call) = &request.call else {
        panic!("Host callback");
    };
    let mut candidate = json!(call.upload_scope.as_ref().unwrap());
    candidate["requestId"] = json!(request.id);
    candidate["threadId"] = json!(call.thread_id);
    candidate["turnId"] = json!(call.turn_id);
    candidate["callId"] = json!(call.call_id);
    candidate["uploadId"] = json!(uuid::Uuid::new_v4().to_string());
    let body = format!("{START}{}{END}", "x".repeat(FILL));
    let items = json!([{"typeTag": 1, "length": body.len().to_string()}]);
    upload(
        peer,
        "begin",
        json!({"candidate": candidate, "success": true, "items": items}),
    )
    .await;
    let mut offset = 0;
    for (index, chunk) in body.as_bytes().chunks(MAX_CHUNK_BYTES).enumerate() {
        upload(
            peer,
            "chunk",
            json!({
                "candidate": candidate, "itemIndex": 0, "sequence": (index + 1).to_string(),
                "offset": offset.to_string(), "dataBase64": STANDARD.encode(chunk),
                "sha256": format!("{:x}", Sha256::digest(chunk)),
            }),
        )
        .await;
        offset += chunk.len();
        if offset > FOREGROUND_TOOL_RESULT_PREVIEW_BYTES
            && let Some(offsets) = &mut progress
        {
            offsets
                .wait_for(|written| *written >= offset as u64)
                .await
                .unwrap();
        }
    }
    let digest = sha2::digest::Output::<Sha256>::from(canonical_digest(&[(1, body.as_bytes())]));
    upload(
        peer,
        "end",
        json!({
            "candidate": candidate, "success": true, "items": items,
            "total": offset.to_string(), "digest": format!("{digest:x}"),
        }),
    )
    .await;
    body.len()
}

async fn create_host_session(
    root: &Path,
    workspace: &Path,
) -> (
    rustx::runtime::identity::SessionId,
    rustx::runtime::identity::ConversationId,
    rustx::runtime::identity::SessionNodeId,
) {
    let (server, url, diagnostics) = start(root).await;
    let mut peer = Peer::connect(&url).await;
    let dynamic_tools = ["business", "inventory"].map(|namespace| HostToolDefinition {
        namespace: Some(namespace.into()),
        name: "lookup".into(),
        description: "Read application record".into(),
        permissions: None,
        input_schema: json!({"type": "object", "properties": {"key": {"type": "string"}}, "required": ["key"], "additionalProperties": false}),
    });
    let MethodResult::SessionTransition {
        thread, session, ..
    } = peer
        .rpc(Method::SessionCreate {
            input_references: None,
            dynamic_tools: dynamic_tools.to_vec(),
            submission: None,
            settings: SessionPersistentState {
                session_config: None,
                session_resources: None,
                cwd: workspace.to_owned(),
                model: None,
            },
        })
        .await
    else {
        panic!("created Host Session");
    };
    peer.rpc(Method::SessionName {
        session_id: session.id.clone(),
        name: "Hosted publication".into(),
    })
    .await;
    drop(peer);
    shutdown(server, diagnostics).await;
    (session.id, thread.id, thread.node_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_published_host_output_cannot_cross_a_local_client_generation() {
    crash_during_output(CrashCut::PublishReceipt).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_result_after_client_death_at_private_save_cuts() {
    for cut in [CrashCut::Allocated, CrashCut::LastWrite, CrashCut::Sealed] {
        crash_during_output(cut).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_death_after_canonical_host_result_cannot_rewrite_saved_history() {
    crash_during_output(CrashCut::Canonical).await;
}

async fn crash_during_output(cut: CrashCut) {
    tokio::time::timeout(Duration::from_secs(90), async {
        let Some(provider) = ProviderEmulator::start("app_server_host_broadcast").await else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        config(root.path(), &workspace, &provider);
        let probe = workspace.join("probe");
        std::fs::write(&probe, b"original Workspace").unwrap();
        let (session, thread, node) = create_host_session(root.path(), &workspace).await;

        let (_bins, binary) = super::super::app_server::isolated_server_pair();
        let scratch = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let witness_path = scratch.path().join("witness");
        let witness = UnixListener::bind(&witness_path).unwrap();
        stage_wrapper(&binary, &witness_path);
        let (start_old, old_started) = oneshot::channel();
        let (start_new, new_started) = oneshot::channel();
        let (saw_save, reached_cut) = oneshot::channel();
        let (offsets, progress) = watch::channel(0);
        let relay = tokio::spawn(async move {
            let (old, (data, server_data), (mut control, mut server_control)) =
                proxy_launch(&witness, scratch.path(), 1).await;
            start_old.send(old).ok().unwrap();
            let old_data =
                tokio::spawn(observe_save_cut(data, server_data, offsets, cut, saw_save));
            let old_control = tokio::spawn(async move {
                let _ = tokio::io::copy_bidirectional(&mut control, &mut server_control).await;
            });
            let (new, (data, server_data), (mut control, mut server_control)) =
                proxy_launch(&witness, scratch.path(), 2).await;
            start_new.send(new).ok().unwrap();
            let _ = tokio::join!(
                reject_save_replay(data, server_data),
                tokio::io::copy_bidirectional(&mut control, &mut server_control),
            );
            old_data.await.unwrap();
            old_control.await.unwrap();
        });
        let mut command = server_command(root.path(), &binary);
        command
            .args([
                "--local-execution-session",
                session.as_str(),
                "--local-execution-workspace",
            ])
            .arg(&workspace);
        let (server, url, diagnostics) = launch(command).await;
        let old = old_started.await.unwrap();
        let mut peer = Peer::connect(&url).await;
        let target = peer.follow(thread.clone()).await;
        let MethodResult::TurnInputAccepted { turn_id, .. } = peer
            .rpc(Method::TurnStart {
                target,
                input_id: "host-publication-at-generation-cut".into(),
                content: vec![UserInputBlock::Text(TextBlock {
                    text: "shared host request".into(),
                })],
            })
            .await
        else {
            panic!("Host attempt acceptance");
        };
        tokio::select! {
            _ = provider.await_gate("host-call") => {}
            () = peer.settled(&turn_id) => panic!("the Attempt ended before the Host call"),
        }
        provider.release_gate("host-call").await;
        let ServerMessage::Request(request) = peer.callback().await else {
            panic!("Host callback");
        };
        let progress = (cut != CrashCut::Allocated).then_some(progress);
        let bytes = upload_result(&mut peer, &request, progress).await;
        assert_eq!(bytes as u64, LENGTH);
        let held = reached_cut.await.unwrap();
        let key = match &held.receipt {
            SaveReply::Begun { key }
            | SaveReply::Chunked { key, .. }
            | SaveReply::Sealed { key, .. }
            | SaveReply::Published { key, .. } => key,
            reply => panic!("observed the actual save receipt: {reply:?}"),
        };
        assert_eq!(
            held.length,
            if cut == CrashCut::Allocated {
                0
            } else {
                LENGTH
            }
        );
        assert_eq!(key.owner.generation, old.admission.generation);
        assert_eq!(key.owner.instance, old.admission.instance);
        let captures = std::fs::read_dir(&old.scratch)
            .unwrap()
            .filter_map(|path| {
                let path = path.unwrap();
                path.file_name()
                    .to_string_lossy()
                    .starts_with("rustx-save-")
                    .then(|| path.path())
            })
            .collect::<Vec<_>>();
        let [capture] = captures.as_slice() else {
            panic!("one Client-owned output at {cut:?}: {captures:?}");
        };
        let [file] = std::fs::read_dir(capture)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(file.metadata().unwrap().len(), held.length);
        if cut == CrashCut::Canonical {
            peer.settled(&turn_id).await;
        }
        kill(old.pid, Signal::SIGKILL).unwrap();

        if cut != CrashCut::Canonical {
            peer.settled(&turn_id).await;
        }
        let new = new_started.await.unwrap();
        assert_ne!(old.pid, new.pid);
        assert_eq!(new.admission.session, old.admission.session);
        assert_eq!(
            new.admission.server_incarnation,
            old.admission.server_incarnation
        );
        assert_ne!(new.admission.instance, old.admission.instance);
        assert!(
            !old.scratch.exists(),
            "the independent guardian removed exact old storage"
        );
        if let Some(late) = held.late_delivery {
            late.resume.send(()).unwrap();
            if let Err(error) = late.observed.await.unwrap() {
                assert!(
                    rustx_execution_protocol::transport::recoverable_lane_loss(&error),
                    "{error}"
                );
            }
        }
        let chunk = loop {
            let response = peer
                .response(Method::FsReadFile {
                    session_id: session.clone(),
                    path: probe.display().to_string(),
                    offset: 0,
                    max_bytes: 64,
                    expected_revision: None,
                })
                .await;
            match response {
                Response::Success(success) => {
                    let MethodResult::FileChunk { chunk } = success.result else {
                        panic!("the replacement reads the original Workspace");
                    };
                    break chunk;
                }
                Response::Failure(failure)
                    if failure.error.data == Some(ErrorData::EnvironmentCapacityOrConflict) =>
                {
                    // Source observation may own the new data writer just after pairing.
                    tokio::task::yield_now().await;
                }
                other @ Response::Failure(_) => {
                    panic!("replacement Workspace read failed: {other:?}")
                }
            }
        };
        assert_eq!(
            STANDARD.decode(chunk.data_base64).unwrap(),
            b"original Workspace"
        );
        assert_eq!(
            provider.requests().await.len(),
            2,
            "one callback and one model continuation"
        );

        drop(peer);
        shutdown(server, diagnostics).await;
        relay.await.unwrap();
        let sessions = SessionController::open(&root.path().join("runtime")).unwrap();
        let access = sessions
            .acquire_session(&session, Some(&node))
            .await
            .unwrap();
        let store = SqliteConversationStore::open_existing(thread, &access.database_path).unwrap();
        let canonical = store.load_canonical().unwrap();
        let tools = canonical
            .iter()
            .filter_map(|message| match message {
                MessageBlock::Tool(tool) => Some(&tool.result),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [result] = tools.as_slice() else {
            panic!("expected one canonical Host result");
        };
        assert_eq!(result.status, ToolExecutionStatus::Success);
        if cut == CrashCut::Canonical {
            assert!(matches!(
                result.managed_output.as_deref(),
                Some(ManagedOutputContinuation::Complete { .. })
            ));
        } else {
            assert!(matches!(
                result.managed_output.as_deref(),
                Some(ManagedOutputContinuation::Unavailable { .. })
            ));
        }
        provider.finish().await;
    })
    .await
    .expect("real local Host output and Client replacement");
}
