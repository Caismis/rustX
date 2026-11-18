//! Real two-process bootstrap tests: the test binary plays the Server via
//! `rustx-execution-server`, and spawns the real `rustx-execution-client`
//! binary over a real Unix socket.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustx_execution_protocol::transport::{read_record, write_record};
use rustx_execution_protocol::{
    BindingCredential, Bootstrap, Capability, ClientInstanceId, EnvironmentId, Generation, Hello,
    HelloReply, Reject, SessionId,
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
    private: tempfile::TempDir,
    // Held only to keep the workspace directory alive.
    _workspace: tempfile::TempDir,
    workspace_path: PathBuf,
    binding: Binding,
    listener: UnixListener,
    bundle: PathBuf,
}

fn rig() -> Rig {
    rig_with(BTreeSet::from([Capability::Connection]))
}

fn rig_with(required_capabilities: BTreeSet<Capability>) -> Rig {
    let (private, private_path) = private_dir();
    let (workspace, workspace_path) = private_dir();
    let socket = private_path.join("client.sock");
    let bundle = private_path.join("bootstrap");
    let mut reserved = reservation(&workspace_path);
    reserved.required_capabilities = required_capabilities;
    let (binding, listener) = Binding::reserve_local(reserved, &socket, &bundle).unwrap();
    Rig {
        private,
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

/// Accepts the client's connection with a deadline, reporting an early
/// client exit instead of hanging on a client that never connects.
fn accept(rig: &Rig, child: &mut Child) -> UnixStream {
    rig.listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + ACCEPT_DEADLINE;
    let stream = loop {
        match rig.listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!(
                        "client exited {status} before connecting: {}",
                        stderr_of(child)
                    );
                }
                assert!(Instant::now() < deadline, "client never connected");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    };
    rig.listener.set_nonblocking(false).unwrap();
    // BSD accepted streams inherit the listener's nonblocking flag.
    stream.set_nonblocking(false).unwrap();
    stream
}

/// The client must not have connected at all: nothing was admitted, spawned,
/// or written.
fn assert_never_connected(rig: &Rig) {
    rig.listener.set_nonblocking(true).unwrap();
    assert_eq!(
        rig.listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
}

#[test]
fn handshake_ping_pong_and_clean_shutdown() {
    let rig = rig();
    let mut child = spawn_client(&rig);

    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();
    assert_eq!(connection.admission().session.as_str(), "session-1",);
    assert_eq!(rig.binding.status().phase, BindingPhase::Live);

    connection.ping().unwrap();

    // RAII: dropping the admitted connection disconnects the binding and
    // closes the socket; the client observes EOF and exits by itself.
    drop(connection);
    assert_eq!(rig.binding.status().phase, BindingPhase::Detached);
    expect_success(&mut child);
}

#[test]
fn credential_replay_after_eof_never_reconnects() {
    let rig = rig();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let connection = rig.binding.accept_local(stream).unwrap().unwrap();
    drop(connection);
    expect_success(&mut child);
    assert_eq!(rig.binding.status().phase, BindingPhase::Detached);

    // The same owner-only file is still valid input, but the binding is
    // detached: re-admission needs recovery proof, not credential replay.
    let mut replay = spawn_client(&rig);
    let stream = accept(&rig, &mut replay);
    let outcome = rig.binding.accept_local(stream).unwrap();
    assert!(matches!(outcome, Err(Reject::RecoveryRequired)));
    assert!(!replay.wait().unwrap().success());
    assert_eq!(rig.binding.status().phase, BindingPhase::Detached);
}

#[test]
fn world_accessible_credential_or_directory_is_denied() {
    let rig = rig();

    // Loosen the credential file: the client refuses before connecting.
    fs::set_permissions(&rig.bundle, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        !client(&rig.bundle, &rig.workspace_path)
            .status()
            .unwrap()
            .success()
    );

    // Restore the file but loosen the private directory instead.
    fs::set_permissions(&rig.bundle, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(rig.private.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        !client(&rig.bundle, &rig.workspace_path)
            .status()
            .unwrap()
            .success()
    );

    assert_never_connected(&rig);
    fs::set_permissions(rig.private.path(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn hardlinked_binding_file_is_denied() {
    let rig = rig();
    fs::hard_link(&rig.bundle, rig.private.path().join("alias")).unwrap();
    assert!(
        !client(&rig.bundle, &rig.workspace_path)
            .status()
            .unwrap()
            .success()
    );
    assert_never_connected(&rig);
}

#[test]
fn rejected_hello_consumes_nothing_and_the_client_does_not_retry() {
    let rig = rig();
    let original = fs::read(&rig.bundle).unwrap();

    // Replace the credential inside the published record with a wrong one.
    let mut file = fs::File::open(&rig.bundle).unwrap();
    let mut bootstrap: Bootstrap = read_record(&mut file).unwrap();
    drop(file);
    bootstrap.credential = BindingCredential::from_bytes([0xef; 32]);
    let mut wire = Vec::new();
    write_record(&mut wire, &bootstrap).unwrap();
    fs::write(&rig.bundle, &wire).unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let outcome = rig.binding.accept_local(stream).unwrap();
    assert!(matches!(outcome, Err(Reject::InvalidCredential)));
    assert!(!child.wait().unwrap().success());

    // The reject consumed nothing: reservation stands, the client did not
    // retry, and no artifacts appeared in the private directory.
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
    rig.listener.set_nonblocking(true).unwrap();
    assert_eq!(
        rig.listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    let entries: Vec<_> = fs::read_dir(rig.private.path()).unwrap().collect();
    assert_eq!(entries.len(), 2);

    // The honest bootstrap still wins the live connection.
    rig.listener.set_nonblocking(false).unwrap();
    fs::write(&rig.bundle, &original).unwrap();
    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let connection = rig.binding.accept_local(stream).unwrap().unwrap();
    drop(connection);
    expect_success(&mut child);
}

#[test]
fn caller_controls_client_shutdown_without_orphan() {
    let rig = rig();
    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let connection = rig.binding.accept_local(stream).unwrap().unwrap();
    assert_eq!(rig.binding.status().phase, BindingPhase::Live);

    // The caller can always terminate and reap the client; no detached
    // process outlives the test.
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    drop(connection);
    assert_eq!(rig.binding.status().phase, BindingPhase::Detached);
}

#[test]
fn read_range_serves_verified_bytes_at_a_sparse_file_boundary() {
    let rig = rig();
    // A sparse file with a marker near EOF: the read is bounded and offset-exact.
    let path = rig.workspace_path.join("blob");
    let marker = b"MARKER!";
    let marker_at = 1024 * 1024 + 13;
    let mut file = fs::File::create(&path).unwrap();
    file.seek(SeekFrom::Start(marker_at)).unwrap();
    file.write_all(marker).unwrap();
    drop(file);

    let expected =
        rustx_confined_fs::FileMetadata::of(&std::fs::File::open(&path).unwrap()).unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();

    let range = connection
        .read_range(Path::new("blob"), marker_at, 64, None)
        .unwrap();
    assert_eq!(range.bytes, marker);
    assert_eq!(range.offset, marker_at);
    assert_eq!(range.next_offset, None);
    assert_eq!(range.metadata.size.get(), expected.size);
    assert_eq!(range.metadata.revision, expected.revision);
    assert_eq!(range.metadata.identity, expected.identity);
    assert_eq!(range.metadata.modified_at_ms, expected.modified_at_ms);

    // A bounded mid-file read returns the exact slice and a live cursor.
    let range = connection
        .read_range(Path::new("blob"), 7, 64, None)
        .unwrap();
    assert_eq!(range.bytes, vec![0; 64]);
    assert_eq!(range.next_offset, Some(7 + 64));
    assert_eq!(range.metadata.revision, expected.revision);

    // Reading exactly at EOF yields empty bytes, the same stamp, and no
    // continuation.
    let range = connection
        .read_range(Path::new("blob"), expected.size, 64, None)
        .unwrap();
    assert_eq!(range.bytes, Vec::<u8>::new());
    assert_eq!(range.offset, expected.size);
    assert_eq!(range.next_offset, None);
    assert_eq!(range.metadata.revision, expected.revision);

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn a_fully_consumed_empty_file_is_a_valid_read() {
    let rig = rig();
    let path = rig.workspace_path.join("empty");
    fs::write(&path, b"").unwrap();
    let expected =
        rustx_confined_fs::FileMetadata::of(&std::fs::File::open(&path).unwrap()).unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();

    let range = connection
        .read_range(Path::new("empty"), 0, 64, None)
        .unwrap();
    assert_eq!(range.bytes, Vec::<u8>::new());
    assert_eq!(range.offset, 0);
    assert_eq!(range.next_offset, None);
    assert_eq!(range.metadata.size.get(), expected.size);
    assert_eq!(range.metadata.revision, expected.revision);
    assert_eq!(range.metadata.identity, expected.identity);

    connection.ping().unwrap();
    drop(connection);
    expect_success(&mut child);
}

#[test]
fn stale_expected_revision_is_a_conflict_not_a_read() {
    let rig = rig();
    let path = rig.workspace_path.join("asset");
    fs::write(&path, b"first").unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();

    let range = connection
        .read_range(Path::new("asset"), 0, 64, None)
        .unwrap();
    fs::write(&path, b"second").unwrap();
    let error = connection
        .read_range(Path::new("asset"), 0, 64, Some(&range.metadata.revision))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);

    // The connection survives an ordinary typed error reply.
    connection.ping().unwrap();
    drop(connection);
    expect_success(&mut child);
}

#[test]
fn symlink_parent_and_outside_paths_are_denied() {
    let rig = rig();
    fs::write(rig.workspace_path.join("inside"), b"ok").unwrap();
    let outside = private_dir();
    fs::write(outside.1.join("secret"), b"no").unwrap();
    std::os::unix::fs::symlink("inside", rig.workspace_path.join("link")).unwrap();
    std::os::unix::fs::symlink(&outside.1, rig.workspace_path.join("escape")).unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();

    for path in [
        PathBuf::from("link"),
        PathBuf::from("escape/secret"),
        PathBuf::from("../escape/secret"),
        PathBuf::from("/etc/hostname"),
        PathBuf::from("./inside"),
        outside.1.join("secret"),
    ] {
        assert!(
            connection.read_range(&path, 0, 64, None).is_err(),
            "{}",
            path.display()
        );
    }

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn oversized_and_zero_requests_are_invalid_not_reads() {
    let rig = rig();
    fs::write(rig.workspace_path.join("asset"), b"bytes").unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();

    for max_bytes in [0, 64 * 1024 + 1] {
        let error = connection
            .read_range(Path::new("asset"), 0, max_bytes, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{max_bytes}");
    }

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn an_unbound_workspace_argument_never_connects() {
    let rig = rig();
    let other = private_dir();

    // A different --workspace than the bootstrap binds: refuse before any
    // socket I/O, even though the file itself is honest.
    assert!(!client(&rig.bundle, &other.1).status().unwrap().success());
    // A lexically equivalent-but-different form must not alias either.
    let dotted = PathBuf::from(format!("{}/.", rig.workspace_path.display()));
    assert!(!client(&rig.bundle, &dotted).status().unwrap().success());

    assert_never_connected(&rig);
}

#[test]
fn revocation_linearizes_before_any_new_request() {
    let rig = rig();
    fs::write(rig.workspace_path.join("asset"), b"bytes").unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();
    connection
        .read_range(Path::new("asset"), 0, 64, None)
        .unwrap();

    rig.binding.revoke();
    let error = connection
        .read_range(Path::new("asset"), 0, 64, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(connection.ping().is_err());

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn a_tampered_bundle_cannot_advertise_capabilities_the_client_lacks() {
    // A reservation demanding a capability the client does not implement:
    // the client claims only what it physically owns and is rejected.
    let rig = rig_with(BTreeSet::from([Capability::Files]));
    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let outcome = rig.binding.accept_local(stream).unwrap();
    assert!(matches!(outcome, Err(Reject::MissingCapabilities)));
    assert!(!child.wait().unwrap().success());
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);

    // And a bundle carrying a foreign `capabilities` field is not a valid
    // record at all: unknown fields are refused before any connect.
    let fresh = rig_with(BTreeSet::from([Capability::Connection]));
    let mut file = fs::File::open(&fresh.bundle).unwrap();
    let mut record: serde_json::Value = read_record(&mut file).unwrap();
    drop(file);
    record["capabilities"] = serde_json::json!(["files"]);
    let mut file = fs::File::create(&fresh.bundle).unwrap();
    write_record(&mut file, &record).unwrap();
    drop(file);
    assert!(
        !client(&fresh.bundle, &fresh.workspace_path)
            .status()
            .unwrap()
            .success()
    );
    assert_never_connected(&fresh);
}

#[test]
fn a_foreign_protocol_version_never_connects() {
    let rig = rig();
    let mut file = fs::File::open(&rig.bundle).unwrap();
    let mut bootstrap: Bootstrap = read_record(&mut file).unwrap();
    drop(file);
    bootstrap.version = rustx_execution_protocol::PROTOCOL_VERSION + 1;
    let mut file = fs::File::create(&rig.bundle).unwrap();
    write_record(&mut file, &bootstrap).unwrap();
    drop(file);

    assert!(
        !client(&rig.bundle, &rig.workspace_path)
            .status()
            .unwrap()
            .success()
    );
    assert_never_connected(&rig);
}

/// T02 proof fragment: one absolute workspace path is read through a
/// physically distinct mount view. Opt-in manual evidence only — run
/// `test-support/prove_execution_mount_view.sh`, which requires Linux and
/// bubblewrap and exports `RUSTX_MOUNT_VIEW_PROOF`. Running it any other way
/// fails loudly instead of claiming a proof that never happened.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "opt-in bwrap proof; run test-support/prove_execution_mount_view.sh"]
fn range_read_from_private_mount_view() {
    assert!(
        std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some(),
        "opt-in proof only: run test-support/prove_execution_mount_view.sh"
    );
    // bwrap is a hard requirement of the proof: its absence fails here
    // rather than silently proving nothing.
    let bwrap = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join("bwrap"))
        .find(|path| {
            path.is_file()
                && fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
        })
        .expect("the mount-view proof requires an executable `bwrap` on PATH");

    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ReadRange,
    ]));
    // The marker lives only in the physical directory that the client's
    // private mount view substitutes for the workspace path.
    let physical = private_dir();
    let marker = b"mount-view-proof-marker";
    fs::write(physical.1.join("marker"), marker).unwrap();

    // `bwrap --ro-bind / / --ro-bind <physical> <workspace> -- <client>`:
    // inside the namespace the same absolute workspace path resolves to a
    // different physical directory than the server sees.
    let mut command = Command::new(bwrap);
    command
        .arg("--ro-bind")
        .arg("/")
        .arg("/")
        .arg("--ro-bind")
        .arg(&physical.1)
        .arg(&rig.workspace_path)
        .arg("--")
        .arg(env!("CARGO_BIN_EXE_rustx-execution-client"))
        .arg("--binding-file")
        .arg(&rig.bundle)
        .arg("--workspace")
        .arg(&rig.workspace_path)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();

    let stream = accept(&rig, &mut child);
    let mut connection = rig.binding.accept_local(stream).unwrap().unwrap();

    // Server side: the bound workspace path holds no marker before the RPC.
    let server_marker = rig.workspace_path.join("marker");
    assert_eq!(
        fs::metadata(&server_marker).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );

    // The client's distinct view still serves the exact bytes through the
    // same absolute workspace path the bootstrap binds.
    let range = connection
        .read_range(Path::new("marker"), 0, 64, None)
        .unwrap();
    assert_eq!(range.bytes, marker);
    assert_eq!(range.offset, 0);
    assert_eq!(range.next_offset, None);

    // And after the RPC the server's view is unchanged: nothing was created,
    // the bytes came from the private mount, not from server filesystem I/O.
    assert_eq!(
        fs::metadata(&server_marker).unwrap_err().kind(),
        io::ErrorKind::NotFound
    );

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn clean_eof_is_success_and_a_truncated_record_is_not() {
    let rig = rig();

    // Clean EOF: the server closes an idle admitted connection.
    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let connection = rig.binding.accept_local(stream).unwrap().unwrap();
    drop(connection);
    expect_success(&mut child);

    // A truncated record mid-stream is a protocol failure, not an idle EOF.
    let fresh = rig_with(BTreeSet::from([Capability::Connection]));
    let mut child = spawn_client(&fresh);
    let stream = accept(&fresh, &mut child);
    let mut raw = stream.try_clone().unwrap();
    let connection = fresh.binding.accept_local(stream).unwrap().unwrap();
    // Promise a record body that never arrives.
    raw.write_all(&10_u32.to_be_bytes()).unwrap();
    raw.write_all(b"{\"pi").unwrap();
    drop(raw);
    drop(connection);
    assert!(!child.wait().unwrap().success());
}

/// A bounded `wait`: the handshake deadline plus slack, then a kill so a
/// misbehaving client can never hang the suite.
fn wait_bounded(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let status = child.wait().unwrap();
            panic!("client did not exit within 15 seconds; killed and reaped: {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Reads the exited child's bounded stderr diagnostic.
fn stderr_of(child: &mut Child) -> String {
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    stderr
}

/// Waits for the honest child bounded and reports its redacted stderr
/// diagnostic when it failed where success was the contract.
fn expect_success(child: &mut Child) {
    let status = wait_bounded(child);
    assert!(
        status.success(),
        "client exited {status}: {}",
        stderr_of(child)
    );
}

#[test]
fn a_missing_handshake_reply_times_out_the_client() {
    let rig = rig();
    let mut child = client(&rig.bundle, &rig.workspace_path)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The listener reads the client's complete Hello — the barrier that
    // proves the handshake started — then just holds the socket. The
    // client's absolute handshake budget expires; it does not hang.
    let mut stream = accept(&rig, &mut child);
    let hello: Hello = read_record(&mut stream).unwrap();
    assert_eq!(hello.session.as_str(), "session-1");
    assert!(!wait_bounded(&mut child).success());
    assert_eq!(
        stderr_of(&mut child),
        "rustx-execution-client: handshake timed out\n"
    );
    drop(stream);
    // No arbitration ever ran: the binding is untouched.
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
}

#[test]
fn a_socket_closed_before_the_reply_fails_the_client() {
    let rig = rig();
    let mut child = client(&rig.bundle, &rig.workspace_path)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stream = accept(&rig, &mut child);
    let _: Hello = read_record(&mut stream).unwrap();
    drop(stream);
    assert_eq!(wait_bounded(&mut child).code(), Some(1));
    assert_eq!(stderr_of(&mut child), "rustx-execution-client: handshake\n");
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
}

#[test]
fn a_partial_admission_delivery_times_out_the_client() {
    let rig = rig();
    let mut child = client(&rig.bundle, &rig.workspace_path)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stream = accept(&rig, &mut child);
    // The client's Hello arrived in full; the reply is only ever a bare
    // length prefix. The held socket keeps the client waiting on the
    // body until the absolute deadline ends the handshake.
    let hello: Hello = read_record(&mut stream).unwrap();
    assert_eq!(hello.session.as_str(), "session-1");
    let mut wire = Vec::new();
    write_record(&mut wire, &HelloReply::Err(Reject::Revoked)).unwrap();
    stream.write_all(&wire[..4]).unwrap();
    assert!(!wait_bounded(&mut child).success());
    assert_eq!(
        stderr_of(&mut child),
        "rustx-execution-client: handshake timed out\n"
    );
    drop(stream);
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
}
