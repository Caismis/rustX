//! Hold the genuine Client Begin reply while the Host sends more fragments.

use rustx::{
    local_runtime::session_runtime_manager::{
        EndpointAdmission, ExecutionListener, LocalExecutionEndpoint, SessionRuntimeManager,
    },
    runtime::identity::SessionId,
};
use rustx_execution_protocol::{
    Bootstrap, BootstrapEndpoint, ClientReply, ClientRequest, PeerHello,
    save::{Cleanup, SaveReply, SaveRequest},
    transport::{MAX_RECORD_BYTES, read_record, write_record},
};
use serde::Serialize;
use std::{
    io,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{UnixListener, UnixStream},
    sync::oneshot,
};

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

pub(super) async fn spawn(
    manager: &SessionRuntimeManager,
    session: &SessionId,
    scratch: &Path,
) -> (tokio::process::Child, SaveBackpressure) {
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
    let child = tokio::process::Command::new(
        std::env::current_exe()
            .unwrap()
            .with_file_name("rustx-execution-client"),
    )
    .arg("--binding-file")
    .arg(bundle.canonicalize().unwrap())
    .arg("--workspace")
    .arg(endpoint.workspace())
    .env("TMPDIR", scratch)
    .kill_on_drop(true)
    .spawn()
    .unwrap();

    let ExecutionListener::Unix(listener) = endpoint.listener() else {
        panic!("expected a Unix listener");
    };
    let listener = listener.try_clone().unwrap();
    listener.set_nonblocking(true).unwrap();
    let listener = UnixListener::from_std(listener).unwrap();
    let (data_peer, data_server) = pair_lane(
        &proxy,
        &listener,
        &socket_path,
        endpoint.clone(),
        EndpointAdmission::Pending,
    )
    .await;
    let (mut control_peer, mut control_server) = pair_lane(
        &proxy,
        &listener,
        &socket_path,
        endpoint.clone(),
        EndpointAdmission::Live,
    )
    .await;
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
            control: endpoint.control().unwrap(),
        },
    )
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
