//! Interleave Host uploads with receipts from a genuine Client.

use rustx::{
    local_runtime::session_runtime_manager::{
        EndpointAdmission, ExecutionListener, LocalExecutionEndpoint, SessionRuntimeManager,
    },
    runtime::identity::SessionId,
};
use rustx_execution_protocol::{
    Bootstrap, BootstrapEndpoint, ClientReply, ClientRequest, PeerHello,
    changes::{ChangeReply, ChangeRequest},
    process::ProcessOperationKey,
    save::{Cleanup, Locator, SaveKey, SaveReply, SaveRequest},
    transport::{MAX_RECORD_BYTES, read_record, write_record},
};
use serde::Serialize;
use std::{
    io,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{UnixListener, UnixStream},
    sync::{oneshot, watch},
};

/// The real data lane acknowledges completed physical writes and disposals.
pub(super) struct SaveReceipts(watch::Receiver<u64>);

impl SaveReceipts {
    pub(super) async fn wait_for_files(&mut self, scratch: &Path, expected: &[u64]) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                self.0.borrow_and_update();
                let mut lengths = super::saved_files(scratch)
                    .into_iter()
                    .filter_map(|file| file.metadata().ok().map(|metadata| metadata.len()))
                    .collect::<Vec<_>>();
                lengths.sort_unstable();
                if lengths == expected {
                    break;
                }
                self.0.changed().await.expect("physical Save receipts");
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Client staging lengths {expected:?}, physical receipt ordinal {}",
                *self.0.borrow()
            )
        });
    }
}

pub(super) async fn observe_save_receipts(
    manager: &SessionRuntimeManager,
    session: &SessionId,
    scratch: &Path,
) -> (tokio::process::Child, SaveReceipts) {
    let (child, relay) = prepare(manager, session, scratch, None).await;
    let ((mut client, mut server), (mut control_peer, mut control_server)) = relay.pair().await;
    tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut control_peer, &mut control_server).await;
    });
    let (observed, receipts) = watch::channel(0);
    tokio::spawn(async move {
        while let Ok(request) = receive::<ClientRequest>(&mut server).await {
            send(&mut client, &request).await;
            let reply = receive::<ClientReply>(&mut client).await.unwrap();
            if matches!(reply, ClientReply::Save(_)) {
                observed.send_modify(|count| *count += 1);
            }
            send(&mut server, &reply).await;
        }
    });
    (child, SaveReceipts(receipts))
}

pub(super) struct SaveBackpressure {
    begun: oneshot::Receiver<()>,
    resume: oneshot::Sender<()>,
    cleaned: oneshot::Receiver<()>,
    control: rustx_execution_server::ControlHandle,
}

impl SaveBackpressure {
    pub(super) async fn wait_for_begin(&mut self) {
        (&mut self.begun).await.unwrap();
    }

    pub(super) async fn check_health_and_release(self) {
        tokio::task::spawn_blocking(move || self.control.health())
            .await
            .unwrap()
            .unwrap();
        self.resume.send(()).unwrap();
        self.cleaned.await.unwrap();
    }
}

async fn receive<T: serde::de::DeserializeOwned>(socket: &mut UnixStream) -> io::Result<T> {
    let size = socket.read_u32().await? as usize;
    assert!((1..=MAX_RECORD_BYTES).contains(&size));
    let mut record = vec![0; size];
    socket.read_exact(&mut record).await?;
    serde_json::from_slice(&record).map_err(io::Error::other)
}

async fn send(socket: &mut UnixStream, value: &impl Serialize) {
    let mut bytes = Vec::new();
    write_record(&mut bytes, value).unwrap();
    socket.write_all(&bytes).await.unwrap();
}

async fn pair_lane(
    proxy: &UnixListener,
    listener: &UnixListener,
    server_path: &Path,
    endpoint: Arc<LocalExecutionEndpoint>,
    expected: EndpointAdmission,
) -> (UnixStream, UnixStream) {
    let (mut client, _) = proxy.accept().await.unwrap();
    let mut server = UnixStream::connect(server_path).await.unwrap();
    let (accepted, _) = listener.accept().await.unwrap();
    let admit = tokio::task::spawn_blocking(move || {
        endpoint.admit_local_stream(accepted.into_std().unwrap())
    });
    let hello = receive::<PeerHello>(&mut client).await.unwrap();
    send(&mut server, &hello).await;
    let reply = receive::<serde_json::Value>(&mut server).await.unwrap();
    send(&mut client, &reply).await;
    assert_eq!(admit.await.unwrap().unwrap(), expected);
    (client, server)
}

struct Relay {
    listener: UnixListener,
    proxy: UnixListener,
    server: PathBuf,
    endpoint: Arc<LocalExecutionEndpoint>,
}

impl Relay {
    async fn pair(&self) -> ((UnixStream, UnixStream), (UnixStream, UnixStream)) {
        let data = pair_lane(
            &self.proxy,
            &self.listener,
            &self.server,
            self.endpoint.clone(),
            EndpointAdmission::Pending,
        )
        .await;
        let control = pair_lane(
            &self.proxy,
            &self.listener,
            &self.server,
            self.endpoint.clone(),
            EndpointAdmission::Live,
        )
        .await;
        (data, control)
    }
}

async fn prepare(
    manager: &SessionRuntimeManager,
    session: &SessionId,
    scratch: &Path,
    disk_limit: Option<&Path>,
) -> (tokio::process::Child, Relay) {
    let endpoint = manager.reserve_local_execution(session).await.unwrap();
    std::fs::set_permissions(scratch, std::fs::Permissions::from_mode(0o700)).unwrap();
    let proxy_path = scratch.canonicalize().unwrap().join("proxy.sock");
    let proxy = UnixListener::bind(&proxy_path).unwrap();
    let mut bootstrap: Bootstrap =
        read_record(&mut std::fs::File::open(endpoint.bootstrap_path()).unwrap()).unwrap();
    let BootstrapEndpoint::Unix { socket_path } = std::mem::replace(
        &mut bootstrap.endpoint,
        BootstrapEndpoint::Unix {
            socket_path: proxy_path,
        },
    ) else {
        panic!("expected a Unix binding");
    };
    let bundle = scratch.join("proxy-binding");
    let mut file = std::fs::OpenOptions::new()
        .mode(0o600)
        .write(true)
        .create_new(true)
        .open(&bundle)
        .unwrap();
    write_record(&mut file, &bootstrap).unwrap();
    drop(file);
    let binary = std::env::current_exe()
        .unwrap()
        .with_file_name("rustx-execution-client");
    let mut command = if let Some(volume) = disk_limit {
        let mut command = tokio::process::Command::new("/usr/bin/bwrap");
        command
            .args(["--unshare-user", "--uid"])
            .arg(nix::unistd::geteuid().as_raw().to_string())
            .args(["--dev-bind", "/", "/", "--size", "8192", "--tmpfs"])
            .arg(volume)
            .args(["--chmod", "0700"])
            .arg(volume)
            .args(["--", "/usr/bin/python3", "-c", VERIFY_DISK_LIMIT])
            .arg(binary)
            .env("TMPDIR", volume);
        command
    } else {
        let mut command = tokio::process::Command::new(binary);
        command.env("TMPDIR", scratch);
        command
    };
    let child = command
        .arg("--binding-file")
        .arg(bundle.canonicalize().unwrap())
        .arg("--workspace")
        .arg(endpoint.workspace())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    let ExecutionListener::Unix(listener) = endpoint.listener() else {
        panic!("expected a Unix listener");
    };
    let listener = listener.try_clone().unwrap();
    listener.set_nonblocking(true).unwrap();
    let listener = UnixListener::from_std(listener).unwrap();
    (
        child,
        Relay {
            listener,
            proxy,
            server: socket_path,
            endpoint,
        },
    )
}

pub(super) async fn spawn(
    manager: &SessionRuntimeManager,
    session: &SessionId,
    scratch: &Path,
) -> (tokio::process::Child, SaveBackpressure) {
    let (child, relay) = prepare(manager, session, scratch, None).await;
    let ((data_peer, data_server), (mut control_peer, mut control_server)) = relay.pair().await;
    tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut control_peer, &mut control_server).await;
    });
    let (parked, begun) = oneshot::channel();
    let (resume, released) = oneshot::channel();
    let (settled, cleaned) = oneshot::channel();
    tokio::spawn(proxy_data(
        data_peer,
        data_server,
        parked,
        released,
        settled,
    ));
    (
        child,
        SaveBackpressure {
            begun,
            resume,
            cleaned,
            control: relay.endpoint.control().unwrap(),
        },
    )
}

pub(super) struct LostPublish {
    observed: oneshot::Receiver<(SaveKey, Locator)>,
    reconnect: oneshot::Sender<()>,
    released: oneshot::Receiver<()>,
}

impl LostPublish {
    pub(super) async fn reattach_after_canonical_commit(self) -> (SaveKey, Locator) {
        let publication = self.observed.await.expect("real Client publication");
        self.reconnect.send(()).unwrap();
        self.released
            .await
            .expect("original Client released its Workspace observation");
        publication
    }
}

pub(super) async fn lose_publication_receipt(
    manager: &SessionRuntimeManager,
    session: &SessionId,
    scratch: &Path,
) -> (tokio::process::Child, LostPublish, SaveReceipts) {
    let (child, relay) = prepare(manager, session, scratch, None).await;
    let ((mut data_peer, mut data_server), (mut control_peer, mut control_server)) =
        relay.pair().await;
    let (observed, publication) = oneshot::channel();
    let (reconnect, reconnecting) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (physical, receipts) = watch::channel(0);
    tokio::spawn(async move {
        let control = tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut control_peer, &mut control_server).await;
        });
        let mut observations = Vec::new();
        let (owner, locator) = loop {
            let request = receive::<ClientRequest>(&mut data_server).await.unwrap();
            send(&mut data_peer, &request).await;
            let reply = receive::<ClientReply>(&mut data_peer).await.unwrap();
            if let (
                ClientRequest::WorkspaceChanges(ChangeRequest::Begin { key }),
                ClientReply::WorkspaceChanges(ChangeReply::Accepted),
            ) = (&request, &reply)
            {
                observations.push(key.clone());
            }
            if matches!(reply, ClientReply::Save(_)) {
                physical.send_modify(|count| *count += 1);
            }
            if let ClientReply::Save(SaveReply::Published { key, locator, .. }) = reply {
                assert!(matches!(
                    request,
                    ClientRequest::Save(SaveRequest::Publish { key: requested, .. })
                        if key == requested
                ));
                break (key, locator);
            }
            send(&mut data_server, &reply).await;
        };
        assert_ne!(observations, [] as [ProcessOperationKey; 0]);
        observed.send((owner, locator)).unwrap();
        // The Server loses the actual publication response. Canonical
        // completion and release of the old observation race with the gap.
        drop(data_server);
        drop(data_peer);
        control.abort();
        let _ = control.await;
        reconnecting
            .await
            .expect("canonical commit before recovery");
        let ((data_peer, data_server), (mut control_peer, mut control_server)) = relay.pair().await;
        let _ = tokio::join!(
            confirm_recovered_release(data_peer, data_server, &observations, release),
            tokio::io::copy_bidirectional(&mut control_peer, &mut control_server)
        );
    });
    (
        child,
        LostPublish {
            observed: publication,
            reconnect,
            released,
        },
        SaveReceipts(receipts),
    )
}

async fn confirm_recovered_release(
    mut client: UnixStream,
    mut server: UnixStream,
    observations: &[ProcessOperationKey],
    released: oneshot::Sender<()>,
) {
    let mut released = Some(released);
    while let Ok(request) = receive::<ClientRequest>(&mut server).await {
        assert!(!matches!(request, ClientRequest::Save(_)), "no Save replay");
        send(&mut client, &request).await;
        let reply = receive::<ClientReply>(&mut client).await.unwrap();
        send(&mut server, &reply).await;
        if let ClientRequest::WorkspaceChanges(ChangeRequest::Release { key }) = request
            && observations.contains(&key)
        {
            assert_eq!(reply, ClientReply::WorkspaceChanges(ChangeReply::Accepted));
            released
                .take()
                .expect("one cleanup receipt")
                .send(())
                .unwrap();
        }
    }
    assert!(released.is_none());
}

// The subprocess checks the mounted volume through the same kernel view
// the unmodified Client uses. No production storage operation is mocked.
const VERIFY_DISK_LIMIT: &str = r"
import errno, os, sys
volume = os.environ['TMPDIR']
capacity = os.statvfs(volume)
assert capacity.f_blocks * capacity.f_frsize == 8192
probe = os.path.join(volume, 'capacity-probe')
with open(probe, 'xb', buffering=0) as stream:
    assert stream.write(b'P' * 8192) == 8192
    try:
        stream.write(b'X')
        raise RuntimeError('temporary volume did not fill')
    except OSError as error:
        assert error.errno == errno.ENOSPC
os.unlink(probe)
os.execv(sys.argv[1], sys.argv[1:])
";

pub(super) async fn spawn_disk_limited(
    manager: &SessionRuntimeManager,
    session: &SessionId,
    scratch: &Path,
) -> (tokio::process::Child, oneshot::Receiver<()>) {
    let volume = scratch.join("disk");
    std::fs::create_dir(&volume).unwrap();
    let (child, relay) = prepare(manager, session, scratch, Some(&volume)).await;
    let ((data, server_data), (mut control, mut server_control)) = relay.pair().await;
    tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut control, &mut server_control).await;
    });
    let (settled, receipt) = oneshot::channel();
    tokio::spawn(confirm_storage_failure(data, server_data, settled));
    (child, receipt)
}

async fn confirm_storage_failure(
    mut client: UnixStream,
    mut server: UnixStream,
    settled: oneshot::Sender<()>,
) {
    use rustx_execution_protocol::save::SaveCode;

    let mut settled = Some(settled);
    let mut owner = None;
    let mut append_failed = false;
    while let Ok(request) = receive::<ClientRequest>(&mut server).await {
        send(&mut client, &request).await;
        let reply = receive::<ClientReply>(&mut client).await.unwrap();
        match (&request, &reply) {
            (
                ClientRequest::Save(SaveRequest::Begin { .. }),
                ClientReply::Save(SaveReply::Begun { key }),
            ) => assert!(owner.replace(key.clone()).is_none()),
            (
                ClientRequest::Save(SaveRequest::Chunk { key, .. }),
                ClientReply::Save(SaveReply::Refused {
                    code: SaveCode::StorageUnavailable,
                }),
            ) => {
                assert_eq!(owner.as_ref(), Some(key));
                assert!(!std::mem::replace(&mut append_failed, true));
            }
            (
                ClientRequest::Save(SaveRequest::Abort { key }),
                ClientReply::Save(SaveReply::Aborted {
                    cleanup: Cleanup::Removed,
                    ..
                }),
            ) => {
                assert_eq!(owner.as_ref(), Some(key));
                assert!(append_failed);
                settled.take().unwrap().send(()).unwrap();
            }
            (ClientRequest::Save(request), _) => {
                panic!("unexpected disk-limited Save: {request:?} {reply:?}")
            }
            _ => {}
        }
        send(&mut server, &reply).await;
    }
    assert!(settled.is_none());
}

async fn proxy_data(
    mut client: UnixStream,
    mut server: UnixStream,
    parked: oneshot::Sender<()>,
    released: oneshot::Receiver<()>,
    settled: oneshot::Sender<()>,
) {
    let mut gate = Some((parked, released));
    let mut cleanup = Some(settled);
    let mut owner = None;
    while let Ok(request) = receive::<ClientRequest>(&mut server).await {
        send(&mut client, &request).await;
        let reply = receive::<ClientReply>(&mut client).await.unwrap();
        match (&request, &reply) {
            (
                ClientRequest::Save(SaveRequest::Begin { .. }),
                ClientReply::Save(SaveReply::Begun { key }),
            ) => {
                let (parked, released) = gate.take().expect("one physical allocation");
                owner = Some(key.clone());
                parked.send(()).unwrap();
                released.await.unwrap();
            }
            (
                ClientRequest::Save(SaveRequest::Abort { key }),
                ClientReply::Save(SaveReply::Aborted {
                    cleanup: Cleanup::Removed,
                    ..
                }),
            ) => {
                assert_eq!(owner.as_ref(), Some(key));
                cleanup
                    .take()
                    .expect("one physical cleanup receipt")
                    .send(())
                    .unwrap();
            }
            (ClientRequest::Save(request), _) => panic!("unexpected save beat: {request:?}"),
            _ => {}
        }
        send(&mut server, &reply).await;
    }
    assert!(gate.is_none() && cleanup.is_none());
}

pub(super) struct PendingSeal {
    sealed: oneshot::Receiver<()>,
    resume: oneshot::Sender<()>,
    cleaned: oneshot::Receiver<()>,
    #[cfg(feature = "test-support")]
    losing_abort: oneshot::Receiver<()>,
    #[cfg(feature = "test-support")]
    release_loser: oneshot::Sender<()>,
}

impl PendingSeal {
    pub(super) async fn wait_for_seal(&mut self) {
        (&mut self.sealed).await.expect("actual Client seal");
        #[cfg(feature = "test-support")]
        (&mut self.losing_abort)
            .await
            .expect("loser abort pending before dispatch");
    }

    pub(super) async fn release_and_confirm_removal(self) {
        self.resume.send(()).unwrap();
        #[cfg(feature = "test-support")]
        self.release_loser.send(()).unwrap();
        self.cleaned
            .await
            .expect("actual Client removed the sealed candidate");
    }
}

pub(super) async fn hold_seal_receipt(
    manager: &SessionRuntimeManager,
    session: &SessionId,
    scratch: &Path,
) -> (tokio::process::Child, PendingSeal, SaveReceipts) {
    let (child, relay) = prepare(manager, session, scratch, None).await;
    let ((mut client, mut server), (mut control_peer, mut control_server)) = relay.pair().await;
    tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut control_peer, &mut control_server).await;
    });
    #[cfg(feature = "test-support")]
    let (losing_abort, release_loser) = relay.endpoint.hold_next_save_abort_for_test();
    let (announce, sealed) = oneshot::channel();
    let (resume, released) = oneshot::channel();
    let (confirm, cleaned) = oneshot::channel();
    let (physical, receipts) = watch::channel(0);
    tokio::spawn(async move {
        let mut gate = Some((announce, released));
        let mut confirm = Some(confirm);
        let mut winner = None;
        let mut allocations = Vec::new();
        let mut removals = Vec::new();
        while let Ok(request) = receive::<ClientRequest>(&mut server).await {
            assert!(
                !matches!(request, ClientRequest::Save(SaveRequest::Publish { .. })),
                "cancellation at the seal gate forbids publication"
            );
            send(&mut client, &request).await;
            let reply = receive::<ClientReply>(&mut client).await.unwrap();
            if matches!(reply, ClientReply::Save(_)) {
                physical.send_modify(|count| *count += 1);
            }
            match (&request, &reply) {
                (
                    ClientRequest::Save(SaveRequest::Begin { .. }),
                    ClientReply::Save(SaveReply::Begun { key }),
                ) => {
                    assert!(!allocations.contains(key) && allocations.len() < 2);
                    allocations.push(key.clone());
                }
                (
                    ClientRequest::Save(SaveRequest::Seal { key, .. }),
                    ClientReply::Save(SaveReply::Sealed { .. }),
                ) => {
                    let (announce, released) = gate.take().expect("exactly one physical seal");
                    assert_eq!(allocations.len(), 2);
                    assert!(allocations.contains(key));
                    #[cfg(feature = "test-support")]
                    assert!(removals.is_empty(), "Seal precedes the losing Abort");
                    winner = Some(key.clone());
                    announce.send(()).unwrap();
                    released.await.unwrap();
                }
                (
                    ClientRequest::Save(SaveRequest::Abort { key }),
                    ClientReply::Save(SaveReply::Aborted {
                        key: receipt_key,
                        cleanup: Cleanup::Removed,
                    }),
                ) => {
                    assert_eq!(receipt_key, key);
                    assert!(allocations.contains(key) && !removals.contains(key));
                    removals.push(key.clone());
                    if removals.len() == 2 {
                        assert_eq!(allocations.len(), 2);
                        assert!(winner.as_ref().is_some_and(|key| removals.contains(key)));
                        confirm
                            .take()
                            .expect("one winner cleanup")
                            .send(())
                            .unwrap();
                    }
                }
                _ => {}
            }
            send(&mut server, &reply).await;
        }
        assert!(gate.is_none() && confirm.is_none());
    });
    (
        child,
        PendingSeal {
            sealed,
            resume,
            cleaned,
            #[cfg(feature = "test-support")]
            losing_abort,
            #[cfg(feature = "test-support")]
            release_loser,
        },
        SaveReceipts(receipts),
    )
}
