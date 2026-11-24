//! Real two-process WebSocket bootstrap tests: the test binary plays the
//! Server via `rustx-execution-server` + `rustx-execution-transport`, and
//! spawns the real `rustx-execution-client` binary over a genuine RFC 6455
//! loopback upgrade — never a UDS substitute. Where a parallel Unix rig
//! exists, the same operations must produce identical record semantics.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustx_execution_protocol::transport::{read_record, write_record};
use rustx_execution_protocol::{
    BindingCredential, Bootstrap, BootstrapEndpoint, Capability, ClientInstanceId, EnvironmentId,
    Generation, Reject, SessionId,
};
use rustx_execution_server::{Binding, BindingPhase, Reservation};

const ACCEPT_DEADLINE: Duration = Duration::from_secs(30);

fn reservation(workspace: &Path) -> Reservation {
    Reservation {
        session: SessionId::new("session-1"),
        environment: EnvironmentId::new("env-1"),
        instance: ClientInstanceId::new("instance-1"),
        generation: Generation::new(NonZeroU64::MIN),
        workspace: workspace.to_path_buf(),
        required_capabilities: BTreeSet::from([Capability::Connection]),
    }
}

/// A private directory on a canonical path: on macOS tempdirs live under
/// `/var`, an alias of `/private/var`, and a symlinked component can never
/// be a confined workspace root.
fn private_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

struct Rig {
    _private: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    workspace_path: PathBuf,
    binding: Binding,
    listener: TcpListener,
    bundle: PathBuf,
}

fn ws_rig() -> Rig {
    let (private, private_path) = private_dir();
    let (workspace, workspace_path) = private_dir();
    let bundle = private_path.join("bootstrap");
    let (binding, listener, _published) = Binding::reserve_websocket(
        reservation(&workspace_path),
        "127.0.0.1:0".parse().unwrap(),
        &bundle,
        None,
    )
    .unwrap();
    let url = format!("wss://{}/execution/v7", listener.local_addr().unwrap());
    let mut file = fs::File::open(&bundle).unwrap();
    let published: Bootstrap = read_record(&mut file).unwrap();
    assert!(matches!(
        published.endpoint,
        BootstrapEndpoint::WebSocket { url: published_url, certificate_der: Some(certificate) }
            if published_url == url && !certificate.is_empty()
    ));
    Rig {
        _private: private,
        _workspace: workspace,
        workspace_path,
        binding,
        listener,
        bundle,
    }
}

fn client(bundle: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustx-execution-client"));
    command
        .arg("--binding-file")
        .arg(bundle)
        .arg("--workspace")
        .arg(workspace)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn spawn_client(rig: &Rig) -> Child {
    client(&rig.bundle, &rig.workspace_path)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Accepts one TCP peer, reporting an early client exit instead of
/// hanging.
fn accept_stream(rig: &Rig, child: &mut Child) -> Result<std::net::TcpStream, String> {
    rig.listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    let stream = loop {
        match rig.listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait().unwrap() {
                    return Err(format!(
                        "client exited {status} before connecting: {}",
                        stderr_of(child)
                    ));
                }
                assert!(Instant::now() < deadline, "client never connected");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    };
    rig.listener.set_nonblocking(false).unwrap();
    Ok(stream)
}

/// Runs the binding's authenticated handshake on one accepted stream —
/// the real RFC 6455 upgrade plus the paired `PeerHello` admission —
/// bounded by the shared handshake deadline.
fn admit_stream(
    rig: &Rig,
    stream: std::net::TcpStream,
    deadline: Instant,
) -> Result<rustx_execution_server::ChannelAccept, String> {
    rig.binding
        .accept_websocket_deadline(stream, deadline)
        .map_err(|error| format!("handshake failed: {error}"))
}

/// Accepts one TCP peer and runs the real RFC 6455 upgrade plus the
/// binding's authenticated Hello handshake, bounded by the shared
/// handshake deadline; reports an early client exit instead of hanging.
/// A `Pending` data half keeps accepting for the pairing control lane —
/// the terminal verdict is the committed pair or the reject.
fn accept(rig: &Rig, child: &mut Child) -> Result<Result<(), Reject>, String> {
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    loop {
        let stream = accept_stream(rig, child)?;
        match admit_stream(rig, stream, deadline)? {
            rustx_execution_server::ChannelAccept::Pending => {}
            rustx_execution_server::ChannelAccept::Live(_) => return Ok(Ok(())),
            rustx_execution_server::ChannelAccept::Rejected(reject) => {
                return Ok(Err(reject));
            }
        }
    }
}

/// Overwrites the bundle with a tampered copy, preserving its owner-only
/// file mode — the client re-reads whatever is on disk.
fn tamper_bundle(rig: &Rig, change: impl FnOnce(&mut Bootstrap)) {
    let mut file = fs::File::open(&rig.bundle).unwrap();
    let mut bootstrap: Bootstrap = read_record(&mut file).unwrap();
    drop(file);
    change(&mut bootstrap);
    let mut wire = Vec::new();
    write_record(&mut wire, &bootstrap).unwrap();
    fs::write(&rig.bundle, &wire).unwrap();
    fs::set_permissions(&rig.bundle, fs::Permissions::from_mode(0o600)).unwrap();
}

fn wait_bounded(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "client did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn expect_success(child: &mut Child) {
    let status = wait_bounded(child);
    assert!(
        status.success(),
        "gracefully closed client must exit 0, got {status}: {}",
        stderr_of(child)
    );
}

/// Revocation tears the stream down abruptly — the client must still
/// exit promptly, but non-zero: it never saw a clean close.
fn expect_terminal_refusal(rig: &Rig, child: &mut Child) {
    assert_eq!(accept(rig, child).unwrap(), Err(Reject::Revoked));
    expect_success(child);
    assert!(!rig.binding.revocation_confirmed());
}

fn stderr_of(child: &mut Child) -> String {
    child.stderr.take().map_or_else(String::new, |mut stderr| {
        let mut out = String::new();
        let _ = stderr.read_to_string(&mut out);
        out
    })
}

#[test]
fn websocket_admits_the_real_client_and_serves_typed_records() {
    let rig = ws_rig();
    // Seeded payload exercising UTF-8, escaping, and a nonexistent path.
    let workspace = &rig.workspace_path;
    fs::write(workspace.join("seed.txt"), "line one\nlínea dos — ☃\n").unwrap();
    let mut child = spawn_client(&rig);
    let mut connection = accept_connection(&rig, &mut child);
    assert_eq!(rig.binding.status().phase, BindingPhase::Live);

    // A real range read through the live connection proves record parity:
    // the Client serves verified bytes over the WS channel.
    let read = connection
        .read_range(Path::new("seed.txt"), 0, 4096, None)
        .expect("range read");
    assert_eq!(read.bytes, "line one\nlínea dos — ☃\n".as_bytes());
    assert_eq!(read.next_offset, None);
    assert_ne!(read.metadata.revision, "");
    connection.ping().expect("ping");

    // EOF detaches the binding; the client exits cleanly.
    drop(connection);
    rig.binding.revoke();
    expect_success(&mut child);
}

/// Accepts and returns the live `LocalConnection` for tests that drive
/// typed dispatch themselves — the data half then the pairing control
/// lane, both inside one handshake budget.
fn accept_connection(rig: &Rig, child: &mut Child) -> rustx_execution_server::LocalConnection {
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    loop {
        let stream = accept_stream(rig, child).expect("client connects");
        match admit_stream(rig, stream, deadline).expect("handshake succeeds") {
            rustx_execution_server::ChannelAccept::Pending => {}
            rustx_execution_server::ChannelAccept::Live(connection) => return connection,
            rustx_execution_server::ChannelAccept::Rejected(reject) => {
                panic!("admission rejected: {reject}")
            }
        }
    }
}

#[test]
fn websocket_read_metadata_and_conditional_write_parity() {
    let rig = ws_rig();
    let workspace = &rig.workspace_path;
    fs::write(workspace.join("seed.txt"), "line one\nlínea dos — ☃\n").unwrap();
    let mut child = spawn_client(&rig);
    let mut connection = accept_connection(&rig, &mut child);

    // Range read: identical record semantics to the Unix path — verified
    // bytes, a source stamp, and no next offset inside one page.
    let read = connection
        .read_range(Path::new("seed.txt"), 0, 4096, None)
        .expect("range read");
    let expected = fs::read(workspace.join("seed.txt")).unwrap();
    assert_eq!(read.bytes, expected);
    assert_eq!(read.next_offset, None);
    assert_ne!(read.metadata.revision, "");

    // Metadata: a missing path is a typed error, never a crash.
    let missing = connection
        .entry_metadata(Path::new("absent.txt"))
        .expect_err("missing file is a typed error");
    assert_eq!(missing.kind(), io::ErrorKind::NotFound, "{missing}");

    // Conditional write: create-then-verify through the two-phase
    // ticketed path — the WS channel carries the whole exchange.
    connection
        .write_file(
            Path::new("written.txt"),
            "ws payload — déjà vu\n".as_bytes(),
            None,
            |_| Ok(()),
        )
        .expect("conditional write");
    assert_eq!(
        fs::read(workspace.join("written.txt")).unwrap(),
        "ws payload — déjà vu\n".as_bytes()
    );
    drop(connection);
    rig.binding.revoke();
    expect_success(&mut child);
}

#[test]
fn websocket_credential_mismatch_leaves_the_binding_reserved() {
    let rig = ws_rig();
    let pristine = fs::read(&rig.bundle).unwrap();
    tamper_bundle(&rig, |bootstrap| {
        bootstrap.credential = BindingCredential::from_bytes([0xef; 32]);
    });
    let mut child = spawn_client(&rig);
    assert_eq!(
        accept(&rig, &mut child).unwrap(),
        Err(Reject::InvalidCredential)
    );
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
    assert!(!child.wait().unwrap().success());

    // A rejected credential never consumes the reservation — restoring
    // the provisioned bundle admits the right Client on the same
    // reserved binding.
    fs::write(&rig.bundle, pristine).unwrap();
    let mut child = spawn_client(&rig);
    let connection = accept_connection(&rig, &mut child);
    assert_eq!(rig.binding.status().phase, BindingPhase::Live);
    drop(connection);
    expect_success(&mut child);
}

#[test]
fn websocket_session_version_and_generation_mismatches_stay_reserved() {
    type Tamper = (&'static str, fn(&mut Bootstrap));
    let tampers: [Tamper; 3] = [
        ("session", |b: &mut Bootstrap| {
            b.session = SessionId::new("session-2");
        }),
        ("generation", |b: &mut Bootstrap| {
            b.generation = Generation::new(NonZeroU64::new(7).unwrap());
        }),
        ("version", |b: &mut Bootstrap| {
            b.version = 1;
        }),
    ];
    for (name, tamper) in tampers {
        let rig = ws_rig();
        tamper_bundle(&rig, tamper);
        let mut child = spawn_client(&rig);
        let outcome = accept(&rig, &mut child);
        if name == "version" {
            // A foreign protocol version is refused client-side before
            // connect — nothing reaches the listener, and the binding is
            // untouched either way.
            assert!(matches!(outcome, Err(ref reason) if reason.contains("exited")));
        } else {
            assert!(matches!(outcome, Ok(Err(_))), "{name}: {outcome:?}");
        }
        assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[test]
fn websocket_plaintext_nonloopback_and_wrong_path_never_connect() {
    let rig = ws_rig();
    // A bundle that violates endpoint policy is rejected client-side
    // before a byte leaves the process — the listener never accepts.
    let address = rig.listener.local_addr().unwrap();
    for url in [
        format!("ws://192.0.2.10:{}/execution/v7", address.port()),
        format!("ws://{address}/execution/v7"),
        format!("wss://{address}/other"),
        format!("wss://user@{address}/execution/v7"),
        format!("wss://{address}/execution/v7?token=1"),
    ] {
        tamper_bundle(&rig, |bootstrap| {
            bootstrap.endpoint = BootstrapEndpoint::WebSocket {
                url: url.clone(),
                certificate_der: None,
            };
        });
        let mut child = client(&rig.bundle, &rig.workspace_path)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert!(
            !wait_bounded(&mut child).success(),
            "client must refuse endpoint {url}"
        );
        rig.listener.set_nonblocking(true).unwrap();
        assert_eq!(
            rig.listener.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "no connection attempt may leave the client for {url}"
        );
        rig.listener.set_nonblocking(false).unwrap();
    }
}

#[test]
fn websocket_refuses_a_different_loopback_tls_identity_before_admission() {
    let rig = ws_rig();
    let unrelated =
        rustx_execution_transport::LoopbackTls::new(rig.listener.local_addr().unwrap().ip())
            .unwrap();
    let mut child = spawn_client(&rig);
    let stream = accept_stream(&rig, &mut child).unwrap();
    assert!(
        unrelated
            .accept(stream, Instant::now() + ACCEPT_DEADLINE)
            .is_err()
    );
    assert!(!wait_bounded(&mut child).success());
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
    let mut authorized = spawn_client(&rig);
    assert_eq!(accept(&rig, &mut authorized), Ok(Ok(())));
    let _ = authorized.wait().unwrap();
}

#[test]
fn websocket_plaintext_never_consumes_the_tls_reservation() {
    let rig = ws_rig();
    // A plaintext peer fails at the TLS handshake; the binding
    // stays Reserved and the real Client still wins afterwards.
    let mut raw = TcpStream::connect(rig.listener.local_addr().unwrap()).unwrap();
    raw.write_all(b"GET /execution/v7 HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    raw.flush().unwrap();
    let (stream, _) = rig.listener.accept().unwrap();
    assert!(
        rig.binding
            .accept_websocket_deadline(stream, Instant::now() + Duration::from_secs(5))
            .is_err()
    );
    drop(raw);
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);

    let mut child = spawn_client(&rig);
    let connection = accept_connection(&rig, &mut child);
    assert_eq!(rig.binding.status().phase, BindingPhase::Live);
    // Orderly server teardown sends a real Close frame: exit 0.
    drop(connection);
    expect_success(&mut child);
}

#[test]
fn websocket_slow_peer_shares_one_absolute_deadline() {
    let rig = ws_rig();
    // A peer that trickles the upgrade request one line at a time must
    // die on the armed deadline, not get a budget per fragment.
    let deadline = Instant::now() + Duration::from_millis(700);
    let mut raw = TcpStream::connect(rig.listener.local_addr().unwrap()).unwrap();
    raw.write_all(b"GET /execution/v7 HTTP/1.1\r\n").unwrap();
    let (stream, _) = rig.listener.accept().unwrap();
    let outcome = rustx_execution_transport::accept(stream, deadline);
    let elapsed = deadline.saturating_duration_since(Instant::now());
    assert!(outcome.is_err(), "a stalled upgrade must fail");
    assert!(
        elapsed.is_zero() || elapsed < Duration::from_millis(200),
        "the deadline bound the trickle: {elapsed:?} remaining"
    );
    drop(raw);
}

#[test]
fn websocket_revocation_wakes_an_idle_client() {
    let rig = ws_rig();
    let mut child = spawn_client(&rig);
    let _connection = accept_connection(&rig, &mut child);
    assert_eq!(rig.binding.status().phase, BindingPhase::Live);
    // Revocation interrupts the idle lane. The listener can provide an
    // authenticated terminal refusal, but has no physical Revoke receipt.
    rig.binding.revoke();
    expect_terminal_refusal(&rig, &mut child);
}

#[test]
fn websocket_recovery_reauthenticates_both_lanes_for_the_same_instance() {
    let rig = ws_rig();
    let mut child = spawn_client(&rig);
    let mut first = accept_connection(&rig, &mut child);
    first.ping().unwrap();
    rig.binding.disconnect(first.connection_id());
    assert_eq!(rig.binding.status().phase, BindingPhase::Detached);

    let mut resumed = accept_connection(&rig, &mut child);
    assert_ne!(resumed.connection_id(), first.connection_id());
    assert_eq!(resumed.admission(), first.admission());
    assert!(first.ping().is_err());
    drop(first);
    resumed.ping().unwrap();
    resumed.control().unwrap().health().unwrap();
    resumed.control().unwrap().request_revoke().unwrap();
    drop(resumed);
    expect_success(&mut child);
}

#[test]
fn websocket_duplicate_launch_never_allows_a_replacement() {
    let rig = ws_rig();
    let mut first = spawn_client(&rig);
    let _connection = accept_connection(&rig, &mut first);
    // A second connect while the first is live is AlreadyConnected —
    // never a replacement.
    let mut second = spawn_client(&rig);
    assert_eq!(
        accept(&rig, &mut second).unwrap(),
        Err(Reject::AlreadyConnected)
    );
    assert!(!second.wait().unwrap().success());
    rig.binding.revoke();
    expect_terminal_refusal(&rig, &mut first);
}
