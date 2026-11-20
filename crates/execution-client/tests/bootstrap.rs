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
    BindingCredential, Bootstrap, Capability, ClientInstanceId, ClientReply, ClientRequest,
    DataHelloReply, EditRefusal, EntryKind, EnvironmentId, Generation, LineEnd,
    MAX_EDIT_FILE_BYTES, PeerHello, Reject, SessionId, TextReplacement,
};
use rustx_execution_server::{Binding, BindingPhase, RefusedEdit, Reservation};

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
    let (binding, listener, _published) =
        Binding::reserve_local(reserved, &socket, &bundle).unwrap();
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
/// Runs the mandatory paired admission for one spawned client: the
/// first accept is the data half (`Pending`), the second is the
/// control lane carrying the pairing challenge (`Live`).
fn admit_pair(rig: &Rig, child: &mut Child) -> rustx_execution_server::LocalConnection {
    let stream = accept(rig, child);
    admit_pair_stream(rig, child, stream)
}

/// The paired admission when the data stream is already in hand — for
/// tests that inspect or clone the data lane itself.
fn admit_pair_stream(
    rig: &Rig,
    child: &mut Child,
    stream: UnixStream,
) -> rustx_execution_server::LocalConnection {
    match rig.binding.accept_local(stream).unwrap() {
        rustx_execution_server::ChannelAccept::Pending => {}
        _ => panic!("expected a pending data half"),
    }
    let control = accept(rig, child);
    match rig.binding.accept_local(control).unwrap() {
        rustx_execution_server::ChannelAccept::Live(connection) => connection,
        _ => panic!("expected a committed pair"),
    }
}

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

    let mut connection = admit_pair(&rig, &mut child);
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
    let connection = admit_pair(&rig, &mut child);
    drop(connection);
    expect_success(&mut child);
    assert_eq!(rig.binding.status().phase, BindingPhase::Detached);

    // The same owner-only file is still valid input, but the binding is
    // detached: re-admission needs recovery proof, not credential replay.
    let mut replay = spawn_client(&rig);
    let stream = accept(&rig, &mut replay);
    let outcome = rig.binding.accept_local(stream).unwrap();
    assert!(matches!(
        outcome,
        rustx_execution_server::ChannelAccept::Rejected(Reject::RecoveryRequired)
    ));
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
    assert!(matches!(
        outcome,
        rustx_execution_server::ChannelAccept::Rejected(Reject::InvalidCredential)
    ));
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
    let connection = admit_pair(&rig, &mut child);
    drop(connection);
    expect_success(&mut child);
}

#[test]
fn caller_controls_client_shutdown_without_orphan() {
    let rig = rig();
    let mut child = spawn_client(&rig);
    let connection = admit_pair(&rig, &mut child);
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
    let mut connection = admit_pair(&rig, &mut child);

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
    let mut connection = admit_pair(&rig, &mut child);

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
    let mut connection = admit_pair(&rig, &mut child);

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
fn read_lines_pages_a_large_file_in_single_bounded_exchanges() {
    let rig = rig();
    // ~1 MiB of short lines: an early small page and a late page each
    // complete as one typed exchange — the client never ships the file.
    let path = rig.workspace_path.join("large.log");
    let mut file = fs::File::create(&path).unwrap();
    for index in 0..60_000u64 {
        writeln!(file, "line{index}").unwrap();
    }
    drop(file);
    let expected =
        rustx_confined_fs::FileMetadata::of(&std::fs::File::open(&path).unwrap()).unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // The early page returns only the requested window and a counted
    // total over the whole — still-validated — source.
    let page = connection
        .read_lines(Path::new("large.log"), 1, Some(3), None)
        .unwrap();
    assert_eq!(page.text, "line0\nline1\nline2");
    assert_eq!(page.shown_lines, 3);
    assert_eq!(page.total_lines, 60_001);
    assert_eq!(page.end, LineEnd::RequestedLimit);
    assert_eq!(page.metadata.revision, expected.revision);
    assert_eq!(page.metadata.size.get(), expected.size);
    assert_eq!(page.metadata.identity, expected.identity);

    // A late page near EOF is one bounded exchange: the six addressed
    // lines plus the source's trailing empty element.
    let page = connection
        .read_lines(
            Path::new("large.log"),
            59_995,
            None,
            Some(&expected.revision),
        )
        .unwrap();
    assert_eq!(
        page.text,
        "line59994\nline59995\nline59996\nline59997\nline59998\nline59999\n"
    );
    assert_eq!(page.shown_lines, 7);
    assert_eq!(page.total_lines, 60_001);
    assert_eq!(page.end, LineEnd::Complete);
    assert_eq!(page.metadata.revision, expected.revision);

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn read_lines_reports_end_reasons_and_keeps_stable_semantics() {
    let rig = rig();
    let path = rig.workspace_path.join("big-line");
    // A first line over the 50KB budget: a diagnostic page, not an error.
    let mut bytes = vec![b'x'; 60 * 1024];
    bytes.extend_from_slice(b"\nrest");
    fs::write(&path, &bytes).unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    let page = connection
        .read_lines(Path::new("big-line"), 1, None, None)
        .unwrap();
    assert_eq!(page.shown_lines, 0);
    assert_eq!(page.text, "");
    assert_eq!(page.total_lines, 2);
    assert_eq!(
        page.end,
        LineEnd::FirstLineTooLarge {
            bytes: rustx_execution_protocol::DecimalU64::new((60 * 1024) as u64)
        }
    );

    // An offset beyond the source is a typed page state, not an error.
    let page = connection
        .read_lines(Path::new("big-line"), 9, None, None)
        .unwrap();
    assert_eq!(page.end, LineEnd::OffsetPastEnd);
    assert_eq!(page.shown_lines, 0);

    // Skipping the oversized line pages the remainder normally.
    let page = connection
        .read_lines(Path::new("big-line"), 2, None, None)
        .unwrap();
    assert_eq!(page.text, "rest");
    assert_eq!(page.end, LineEnd::Complete);

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn read_lines_stale_revision_is_a_conflict_not_a_read() {
    let rig = rig();
    let path = rig.workspace_path.join("asset");
    fs::write(&path, b"first\n").unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    let page = connection
        .read_lines(Path::new("asset"), 1, None, None)
        .unwrap();
    fs::write(&path, b"second\n").unwrap();
    let error = connection
        .read_lines(Path::new("asset"), 1, None, Some(&page.metadata.revision))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);

    // The connection survives an ordinary typed error reply.
    connection.ping().unwrap();
    drop(connection);
    expect_success(&mut child);
}

#[test]
fn read_lines_refuses_non_utf8_and_keeps_the_connection_live() {
    let rig = rig();
    let path = rig.workspace_path.join("binary");
    fs::write(&path, b"text\xFFtail").unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // A non-UTF-8 source is a semantic refusal — InvalidData, never a
    // fabrication — and it never retires the connection.
    let error = connection
        .read_lines(Path::new("binary"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    connection.ping().unwrap();

    // Invalid bytes after the requested page are still the refusal: the
    // scanner validates the whole source, not only the shown window.
    fs::write(&path, b"ok\n\xFF").unwrap();
    let error = connection
        .read_lines(Path::new("binary"), 1, Some(1), None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    // The same connection still serves a clean page afterward.
    fs::write(&path, b"clean\n").unwrap();
    let page = connection
        .read_lines(Path::new("binary"), 1, None, None)
        .unwrap();
    assert_eq!(page.text, "clean\n");
    assert_eq!(page.end, LineEnd::Complete);

    drop(connection);
    expect_success(&mut child);
}

/// One committed document-fixture corpus file copied into the test
/// workspace under the same leaf name.
fn copy_document_fixture(rig: &Rig, fixture: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/read/documents")
        .join(fixture);
    let target = rig
        .workspace_path
        .join(Path::new(fixture).file_name().unwrap());
    fs::copy(&root, &target).unwrap();
    target
}

#[test]
fn read_document_decodes_every_whitelisted_format() {
    let rig = rig();
    // Every whitelisted format decodes through the real client into the
    // same deterministic Markdown the native Read tool projects.
    let fixtures: &[(&str, &[&str])] = &[
        (
            "pdf/small-text.pdf",
            &[
                "The rustX document projection.",
                "Hello from a text-layer PDF.",
            ],
        ),
        (
            "docx/small.docx",
            &["## Quarterly Report", "Revenue grew", "| North | 120 |"],
        ),
        (
            "xlsx/small.xlsx",
            &["## Summary", "| Total revenue | 215 |"],
        ),
        (
            "pptx/small.pptx",
            &["## Project Kickoff", "## Budget Review"],
        ),
    ];
    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    for (fixture, fragments) in fixtures {
        let target = copy_document_fixture(&rig, fixture);
        let name = target.file_name().unwrap().to_owned();
        let expected =
            rustx_confined_fs::FileMetadata::of(&std::fs::File::open(&target).unwrap()).unwrap();
        let page = connection
            .read_document(Path::new(&name), 1, None, Some(&expected.revision))
            .unwrap();
        assert!(!page.text.is_empty(), "{fixture} produced no page");
        assert!(page.shown_lines >= 1);
        assert!(page.total_lines >= 1);
        assert_eq!(page.end, LineEnd::Complete, "{fixture}");
        // The logical source is the decoded Markdown, independent of the
        // compressed bytes the stamp describes.
        assert!(page.logical_bytes > 0);
        assert_eq!(page.source.revision, expected.revision);
        assert_eq!(page.source.identity, expected.identity);
        assert_eq!(page.source.size.get(), expected.size);
        for fragment in *fragments {
            assert!(
                page.text.contains(fragment),
                "{fixture} page missing {fragment:?}: {}",
                page.text
            );
        }
    }

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn read_document_serves_deterministic_pages_and_stale_revisions_conflict() {
    let rig = rig();
    let target = copy_document_fixture(&rig, "docx/larger.docx");
    let name = target.file_name().unwrap().to_owned();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // One bounded exchange pages the decoded text: a limited window is a
    // typed page, never the whole projection or the binary source.
    let page = connection
        .read_document(Path::new(&name), 1, Some(2), None)
        .unwrap();
    assert_eq!(page.shown_lines, 2);
    assert_eq!(page.end, LineEnd::RequestedLimit);
    assert!(page.total_lines > 2);
    let logical = page.logical_bytes;
    assert!(logical > page.text.len() as u64);

    // The projection is deterministic: the identical request returns
    // byte-for-byte the same page and the same stamp.
    let again = connection
        .read_document(Path::new(&name), 1, Some(2), None)
        .unwrap();
    assert_eq!(page, again);

    // A stale expected revision is a conflict, never a served page —
    // and the connection stays live.
    fs::write(&target, b"changed").unwrap();
    let error = connection
        .read_document(Path::new(&name), 1, None, Some(&page.source.revision))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    connection.ping().unwrap();

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn read_document_refuses_unsupported_corrupt_oversized_and_textless() {
    let rig = rig();
    // A non-whitelisted source, a corrupt document, a sparse over-cap
    // source, and a valid document with no text layer — each is a
    // distinct typed refusal, never a page and never a disconnect.
    fs::write(rig.workspace_path.join("notes.md"), b"# notes\n").unwrap();
    fs::write(rig.workspace_path.join("legacy.doc"), b"old\n").unwrap();
    fs::write(rig.workspace_path.join("broken.docx"), vec![b'x'; 4096]).unwrap();
    let huge = rig.workspace_path.join("huge.xlsx");
    let file = fs::File::create(&huge).unwrap();
    file.set_len(rustx_execution_protocol::MAX_DOCUMENT_SOURCE_BYTES as u64 + 1)
        .unwrap();
    drop(file);
    copy_document_fixture(&rig, "pdf/scanned-no-text.pdf");

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    for path in [Path::new("notes.md"), Path::new("legacy.doc")] {
        let error = connection.read_document(path, 1, None, None).unwrap_err();
        assert_eq!(
            error.kind(),
            io::ErrorKind::Unsupported,
            "{}",
            path.display()
        );
    }
    let error = connection
        .read_document(Path::new("broken.docx"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    // The over-cap source is refused by the stamped-size bound: the
    // parser never ran, proven by the deterministic FileTooLarge kind.
    let error = connection
        .read_document(Path::new("huge.xlsx"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::FileTooLarge);
    let error = connection
        .read_document(Path::new("scanned-no-text.pdf"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("text layer"), "{error}");

    // Every refusal left the connection live and still serving.
    let page = connection
        .read_document(Path::new("notes.md"), 1, None, None)
        .unwrap_err();
    assert_eq!(page.kind(), io::ErrorKind::Unsupported);
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
    let mut connection = admit_pair(&rig, &mut child);

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
    let mut connection = admit_pair(&rig, &mut child);

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
fn metadata_and_directory_cover_root_and_leaf_symlinks() {
    let rig = rig();
    fs::write(rig.workspace_path.join("file"), b"bytes").unwrap();
    fs::create_dir(rig.workspace_path.join("sub")).unwrap();
    std::os::unix::fs::symlink("file", rig.workspace_path.join("link")).unwrap();
    // A symlinked intermediate component must still be refused.
    let outside = private_dir();
    fs::write(outside.1.join("secret"), b"no").unwrap();
    std::os::unix::fs::symlink(&outside.1, rig.workspace_path.join("escape")).unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // The root itself answers with a directory stamp and pages its names
    // sorted, leaf symlinks reported — never followed.
    let root = connection.entry_metadata(Path::new("")).unwrap();
    assert_eq!(root.kind, EntryKind::Directory);
    let page = connection
        .directory_page(Path::new(""), 0, 16, None)
        .unwrap();
    let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["escape", "file", "link", "sub"]);
    assert_eq!(page.next_offset, None);
    let kind_of = |name: &str| {
        page.entries
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .metadata
            .kind
    };
    assert_eq!(kind_of("escape"), EntryKind::Symlink);
    assert_eq!(kind_of("file"), EntryKind::RegularFile);
    assert_eq!(kind_of("link"), EntryKind::Symlink);
    assert_eq!(kind_of("sub"), EntryKind::Directory);

    // A leaf symlink's own metadata is allowed.
    assert_eq!(
        connection.entry_metadata(Path::new("link")).unwrap().kind,
        EntryKind::Symlink
    );
    assert_eq!(
        connection.entry_metadata(Path::new("sub")).unwrap().kind,
        EntryKind::Directory
    );

    // Traversal, symlinked intermediates, absolute and embedded-`.`
    // paths are refused — the link itself is never followed. A raw `.`
    // is refused on the wire too: only the endpoint's lexical helper may
    // spell the root as `.`, normalizing to the empty path first.
    for path in [
        "../outside",
        "escape/secret",
        "/etc",
        ".",
        "sub/../file",
        "sub/./file",
    ] {
        assert!(
            connection.entry_metadata(Path::new(path)).is_err(),
            "{path}"
        );
        assert!(
            connection
                .directory_page(Path::new(path), 0, 16, None)
                .is_err(),
            "{path}"
        );
    }
    // A regular file is not pageable as a directory: the physical
    // ENOTDIR surfaces as the stable `not-found` wire answer.
    assert_eq!(
        connection
            .directory_page(Path::new("file"), 0, 16, None)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    for limit in [0, 129] {
        assert_eq!(
            connection
                .directory_page(Path::new(""), 0, limit, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput,
            "{limit}"
        );
    }

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn directory_pages_bound_the_listing_and_reject_stale_revisions() {
    let rig = rig();
    for name in ["a", "b", "c"] {
        fs::write(rig.workspace_path.join(name), name.as_bytes()).unwrap();
    }

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // A bound smaller than the directory pages with a continuation
    // cursor; resuming returns the remainder and no cursor.
    let first = connection
        .directory_page(Path::new(""), 0, 2, None)
        .unwrap();
    assert_eq!(
        first
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(first.next_offset, Some(2));
    let rest = connection
        .directory_page(Path::new(""), 2, 16, None)
        .unwrap();
    assert_eq!(
        rest.entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["c"]
    );
    assert_eq!(rest.next_offset, None);

    // A membership change between pages makes the earlier revision
    // stale: the expected revision conflicts like a file's does.
    fs::write(rig.workspace_path.join("d"), b"d").unwrap();
    let error = connection
        .directory_page(Path::new(""), 0, 16, Some(&first.revision))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    connection.ping().unwrap();

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
    let mut connection = admit_pair(&rig, &mut child);
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
    assert!(matches!(
        outcome,
        rustx_execution_server::ChannelAccept::Rejected(Reject::MissingCapabilities)
    ));
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

    let mut connection = admit_pair(&rig, &mut child);

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
    let connection = admit_pair(&rig, &mut child);
    drop(connection);
    expect_success(&mut child);

    // A truncated record mid-stream is a protocol failure, not an idle EOF.
    let fresh = rig_with(BTreeSet::from([Capability::Connection]));
    let mut child = spawn_client(&fresh);
    let stream = accept(&fresh, &mut child);
    let mut raw = stream.try_clone().unwrap();
    let connection = admit_pair_stream(&fresh, &mut child, stream);
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
    let PeerHello::Data(hello) = read_record(&mut stream).unwrap() else {
        panic!("the first lane declares the data half");
    };
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
    let _: PeerHello = read_record(&mut stream).unwrap();
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
    let PeerHello::Data(hello) = read_record(&mut stream).unwrap() else {
        panic!("the first lane declares the data half");
    };
    assert_eq!(hello.session.as_str(), "session-1");
    let mut wire = Vec::new();
    write_record(&mut wire, &DataHelloReply::Err(Reject::Revoked)).unwrap();
    stream.write_all(&wire[..4]).unwrap();
    assert!(!wait_bounded(&mut child).success());
    assert_eq!(
        stderr_of(&mut child),
        "rustx-execution-client: handshake timed out\n"
    );
    drop(stream);
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
}

#[test]
fn a_conditional_write_commits_reads_back_and_conflicts_on_stale_revision() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalWrite,
    ]));
    fs::write(rig.workspace_path.join("file"), b"before").unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // The authorized gate runs on the prepared original: a real stamp,
    // never a re-stat by the Server.
    let first = connection
        .write_file(Path::new("file"), b"after", None, |original| {
            assert!(original.is_none());
            Ok(())
        })
        .unwrap_err();
    assert_eq!(first.kind(), io::ErrorKind::AlreadyExists);

    let source_stamp = rustx_confined_fs::FileMetadata::of(
        &std::fs::File::open(rig.workspace_path.join("file")).unwrap(),
    )
    .unwrap();
    let written = connection
        .write_file(
            Path::new("file"),
            b"after",
            Some(&source_stamp.revision),
            |original| {
                // The gate sees the retained prepared original — the
                // exact revision the request carried.
                assert_eq!(original.unwrap().revision, source_stamp.revision);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(fs::read(rig.workspace_path.join("file")).unwrap(), b"after");

    // Read-after-write on the same connection serves the new bytes.
    let range = connection
        .read_range(Path::new("file"), 0, 64, Some(&written.revision))
        .unwrap();
    assert_eq!(range.bytes, b"after");

    // A stale expected revision is a conflict, not a write.
    let stale = connection
        .write_file(
            Path::new("file"),
            b"third",
            Some(&source_stamp.revision),
            |_| panic!("a conflicting write never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(stale.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(rig.workspace_path.join("file")).unwrap(), b"after");

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn a_denied_gate_aborts_the_stage_and_leaves_the_target_untouched() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalWrite,
    ]));
    fs::write(rig.workspace_path.join("file"), b"before").unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // The prepare must succeed first: expected the real revision so the
    // staged preparation exists when the gate denies it.
    let original = rustx_confined_fs::FileMetadata::of(
        &std::fs::File::open(rig.workspace_path.join("file")).unwrap(),
    )
    .unwrap();
    let error = connection
        .write_file(
            Path::new("file"),
            b"staged",
            Some(&original.revision),
            |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "gate")),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    // The abort removed the private stage: no `.rustx-write-*` sibling
    // survives and the target bytes are untouched.
    let staged: Vec<_> = fs::read_dir(&rig.workspace_path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_str().unwrap().starts_with(".rustx-write-"))
        .collect();
    assert_eq!(staged, Vec::<std::ffi::OsString>::new());
    assert_eq!(
        fs::read(rig.workspace_path.join("file")).unwrap(),
        b"before"
    );

    // The aborted connection stays live: a fresh create-only write
    // commits on the same ticket scope.
    let metadata = connection
        .write_file(Path::new("created"), b"new", None, |_| Ok(()))
        .unwrap();
    assert_eq!(
        fs::read(rig.workspace_path.join("created")).unwrap(),
        b"new"
    );
    assert_eq!(
        fs::metadata(rig.workspace_path.join("created"))
            .map(|m| m.len())
            .unwrap(),
        metadata.size.get()
    );

    drop(connection);
    expect_success(&mut child);
}

#[test]
fn a_revoked_binding_after_prepare_leaves_no_stage_and_no_commit() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalWrite,
    ]));
    fs::write(rig.workspace_path.join("file"), b"before").unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // The caller gate revokes the binding after the prepare but before
    // the commit is dispatched: zero commit frames cross, the physical
    // stream is shut down so the idle client exits, and its unpublished
    // private stage is cleaned up by the dropped owner. The prepare must
    // succeed first — the real revision is expected so the staged
    // preparation exists when the revoke lands.
    let original = rustx_confined_fs::FileMetadata::of(
        &std::fs::File::open(rig.workspace_path.join("file")).unwrap(),
    )
    .unwrap();
    let error = connection
        .write_file(
            Path::new("file"),
            b"staged",
            Some(&original.revision),
            |_| {
                rig.binding.revoke();
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    drop(connection);
    expect_success(&mut child);
    let staged: Vec<_> = fs::read_dir(&rig.workspace_path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_str().unwrap().starts_with(".rustx-write-"))
        .collect();
    assert_eq!(staged, Vec::<std::ffi::OsString>::new());
    assert_eq!(
        fs::read(rig.workspace_path.join("file")).unwrap(),
        b"before"
    );
}

/// The real Client edits through the same ticketed publish kernel as a
/// write: `PrepareEdit` retains and reads the original by descriptor,
/// applies the bounded text-edit algorithm, stages privately, and the
/// Server's gate authorizes the exact prepared stamp before `CommitWrite`
/// crosses.
#[test]
fn a_conditional_edit_commits_bom_crlf_and_multiple_edits() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalEdit,
    ]));
    let original = "\u{FEFF}alpha\r\n'keep'\r\n\u{2018}gamma\u{2019}\r\n";
    fs::write(rig.workspace_path.join("file"), original).unwrap();
    let stamp = rustx_confined_fs::FileMetadata::of(
        &std::fs::File::open(rig.workspace_path.join("file")).unwrap(),
    )
    .unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    let published = connection
        .edit_file(
            Path::new("file"),
            &stamp.revision,
            &[
                TextReplacement {
                    old_text: "alpha".to_owned(),
                    new_text: "A".to_owned(),
                },
                // A fuzzy anchor: the curly quotes never appear literally.
                TextReplacement {
                    old_text: "'gamma'".to_owned(),
                    new_text: "G".to_owned(),
                },
            ],
            |prepared| {
                assert_eq!(prepared.revision, stamp.revision);
                Ok(())
            },
        )
        .unwrap();
    let updated = fs::read_to_string(rig.workspace_path.join("file")).unwrap();
    assert_eq!(updated, "\u{FEFF}A\r\n'keep'\r\nG\r\n");
    assert_eq!(published.size.get(), updated.len() as u64);

    drop(connection);
    expect_success(&mut child);
}

/// Two ticketed preparations on one connection decide independently:
/// committing the write never publishes the aborted edit, and aborting
/// the edit never disturbs the committed write.
#[test]
fn parallel_prepared_mutations_commit_and_abort_independently() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalWrite,
        Capability::ConditionalEdit,
    ]));
    fs::write(
        rig.workspace_path.join("edit-me"),
        "before
",
    )
    .unwrap();
    let stamp = rustx_confined_fs::FileMetadata::of(
        &std::fs::File::open(rig.workspace_path.join("edit-me")).unwrap(),
    )
    .unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);
    let deadline = Instant::now() + Duration::from_secs(10);

    let write = connection
        .prepare_write(
            Path::new("new-file"),
            &rustx_execution_protocol::WriteExpectation::Absent,
            b"created",
            Some(deadline),
            &|| Ok(()),
            &|| (),
        )
        .unwrap();
    let edit = connection
        .prepare_edit(
            Path::new("edit-me"),
            &rustx_execution_protocol::WriteExpectation::Revision(stamp.revision.clone()),
            &[TextReplacement {
                old_text: "before".to_owned(),
                new_text: "after".to_owned(),
            }],
            Some(deadline),
            &|| Ok(()),
            &|| (),
        )
        .unwrap();
    assert_ne!(write.ticket, edit.ticket);

    // Commit one, abort the other — neither decision leaks across tickets.
    let published = connection
        .commit_prepared(&write, Some(deadline), &|| Ok(()), &|| ())
        .unwrap();
    connection
        .abort_prepared(&edit, Some(deadline), &|| Ok(()), &|| ())
        .unwrap();

    drop(connection);
    expect_success(&mut child);
    assert_eq!(
        fs::read(rig.workspace_path.join("new-file")).unwrap(),
        b"created"
    );
    assert_eq!(published.size.get(), 7);
    // The aborted edit never published — the original is untouched.
    assert_eq!(
        fs::read_to_string(rig.workspace_path.join("edit-me")).unwrap(),
        "before
"
    );
    // No staging residue survives the abort.
    let staged: Vec<_> = fs::read_dir(&rig.workspace_path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_str().unwrap().starts_with(".rustx-write-"))
        .collect();
    assert_eq!(staged, Vec::<std::ffi::OsString>::new());
}

/// A chunked write over the wire chunk limit transfers deterministically
/// and the `Written` receipt proves the exact submitted size.
#[test]
fn a_write_over_the_chunk_limit_prepares_and_commits() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalWrite,
    ]));
    let payload = "chunked-content
"
    .repeat(10_000); // ~156 KiB > 64 KiB
    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    let published = connection
        .write_file(Path::new("big"), payload.as_bytes(), None, |_| Ok(()))
        .unwrap();

    drop(connection);
    expect_success(&mut child);
    let on_disk = fs::read(rig.workspace_path.join("big")).unwrap();
    assert_eq!(on_disk, payload.as_bytes());
    assert_eq!(published.size.get(), payload.len() as u64);
}

/// Every semantic `EditRefused` is a typed reason only — the connection
/// stays live, nothing is staged, and no path or anchor text crosses.
#[test]
fn edit_refusals_are_typed_and_the_connection_stays_live() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalEdit,
    ]));
    fs::write(rig.workspace_path.join("file"), "dup dup\n").unwrap();
    fs::write(
        rig.workspace_path.join("huge"),
        "x".repeat(MAX_EDIT_FILE_BYTES + 1),
    )
    .unwrap();
    fs::write(rig.workspace_path.join("binary"), [0xffu8, 0xfe, 0x00]).unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);
    let edit = |old: &str, new: &str| TextReplacement {
        old_text: old.to_owned(),
        new_text: new.to_owned(),
    };
    let stamp_of = |name: &str| {
        rustx_confined_fs::FileMetadata::of(
            &std::fs::File::open(rig.workspace_path.join(name)).unwrap(),
        )
        .unwrap()
    };
    let refused = |error: io::Error| {
        error
            .get_ref()
            .unwrap()
            .downcast_ref::<RefusedEdit>()
            .map(|refused| refused.0)
    };

    let ambiguous = connection
        .edit_file(
            Path::new("file"),
            &stamp_of("file").revision,
            &[edit("dup", "x")],
            |_| panic!("a refusal never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(
        refused(ambiguous),
        Some(EditRefusal::Ambiguous {
            index: 0,
            occurrences: 2
        })
    );

    let no_change = connection
        .edit_file(
            Path::new("file"),
            &stamp_of("file").revision,
            &[edit("dup dup", "dup dup")],
            |_| panic!("a refusal never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(refused(no_change), Some(EditRefusal::NoChange));

    let oversized = connection
        .edit_file(
            Path::new("huge"),
            &stamp_of("huge").revision,
            &[edit("x", "y")],
            |_| panic!("a refusal never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(refused(oversized), Some(EditRefusal::TooLarge));

    // Non-UTF-8 is a typed error, not an edit refusal.
    let binary = connection
        .edit_file(
            Path::new("binary"),
            &stamp_of("binary").revision,
            &[edit("x", "y")],
            |_| panic!("a refusal never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(binary.kind(), io::ErrorKind::InvalidData);

    // Nothing was ever staged and the connection is still live.
    let staged: Vec<_> = fs::read_dir(&rig.workspace_path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_str().unwrap().starts_with(".rustx-write-"))
        .collect();
    assert_eq!(staged, Vec::<std::ffi::OsString>::new());
    connection.ping().unwrap();

    drop(connection);
    expect_success(&mut child);
}

/// A denied authorization aborts the edit's private stage: the target is
/// untouched and no `.rustx-write-*` sibling survives. A target replaced
/// between prepare and commit is an honest conflict, never a publish.
#[test]
fn a_denied_edit_gate_aborts_and_a_replaced_target_conflicts() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalEdit,
    ]));
    fs::create_dir(rig.workspace_path.join("sub")).unwrap();
    fs::write(rig.workspace_path.join("sub").join("file"), "before").unwrap();
    let stamp = rustx_confined_fs::FileMetadata::of(
        &std::fs::File::open(rig.workspace_path.join("sub").join("file")).unwrap(),
    )
    .unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);
    let edit = || TextReplacement {
        old_text: "before".to_owned(),
        new_text: "after".to_owned(),
    };

    // Denied gate: abort removes the stage, file untouched, live on.
    let denied = connection
        .edit_file(Path::new("sub/file"), &stamp.revision, &[edit()], |_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "gate"))
        })
        .unwrap_err();
    assert_eq!(denied.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        fs::read(rig.workspace_path.join("sub").join("file")).unwrap(),
        b"before"
    );
    connection.ping().unwrap();

    // The leaf replaced between prepare and commit: the publish's
    // recheck is a conflict and the outside bytes win.
    let workspace = rig.workspace_path.clone();
    let conflict = connection
        .edit_file(Path::new("sub/file"), &stamp.revision, &[edit()], |_| {
            fs::write(workspace.join("sub").join("file"), "outside").unwrap();
            Ok(())
        })
        .unwrap_err();
    assert_eq!(conflict.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs::read(rig.workspace_path.join("sub").join("file")).unwrap(),
        b"outside"
    );
    connection.ping().unwrap();

    // The ancestor swapped between prepare and commit: the retained
    // parent no longer names the live path, so the publish refuses.
    let restamped = rustx_confined_fs::FileMetadata::of(
        &std::fs::File::open(rig.workspace_path.join("sub").join("file")).unwrap(),
    )
    .unwrap();
    let workspace = rig.workspace_path.clone();
    let swapped = connection
        .edit_file(
            Path::new("sub/file"),
            &restamped.revision,
            &[TextReplacement {
                // The anchor must match the current "outside" bytes — a
                // missing anchor is an honest refusal before the gate.
                old_text: "outside".to_owned(),
                new_text: "after".to_owned(),
            }],
            |_| {
                fs::rename(workspace.join("sub"), workspace.join("old-sub")).unwrap();
                fs::create_dir(workspace.join("sub")).unwrap();
                Ok(())
            },
        )
        .unwrap_err();
    // The swapped ancestor is the kernel's explicit "authorized
    // directory replaced" denial — a pre-commit refusal, never a
    // publish.
    assert_eq!(swapped.kind(), io::ErrorKind::PermissionDenied);
    let sub_entries: Vec<_> = fs::read_dir(rig.workspace_path.join("sub"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(sub_entries, Vec::<std::ffi::OsString>::new());
    // The live bytes moved with the renamed directory, untouched.
    assert_eq!(
        fs::read(rig.workspace_path.join("old-sub").join("file")).unwrap(),
        b"outside"
    );

    let staged: Vec<_> = fs::read_dir(&rig.workspace_path)
        .unwrap()
        .flat_map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                fs::read_dir(path)
                    .unwrap()
                    .map(|inner| inner.unwrap().file_name())
                    .collect()
            } else {
                Vec::<std::ffi::OsString>::new()
            }
        })
        .filter(|name| name.to_str().unwrap_or("").starts_with(".rustx-write-"))
        .collect();
    assert_eq!(staged, Vec::<std::ffi::OsString>::new());

    drop(connection);
    expect_success(&mut child);
}

/// A malformed `PrepareEdit` on the wire — an empty replacement set the
/// typed request could still carry — is a stable `Error`, never a stage,
/// and the connection stays live.
#[test]
fn a_malformed_edit_request_is_an_error_not_a_stage() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalEdit,
    ]));
    fs::write(rig.workspace_path.join("file"), "before").unwrap();

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    // A raw sibling handle on the admitted socket: the typed request
    // reaches the real Client's dispatch without a LocalConnection
    // framing it.
    let mut peer = stream.try_clone().unwrap();
    let connection = admit_pair_stream(&rig, &mut child, stream);

    write_record(
        &mut peer,
        &ClientRequest::PrepareEdit {
            relative_path: Path::new("file").to_path_buf(),
            expectation: rustx_execution_protocol::WriteExpectation::Revision("a".repeat(64)),
            replacements: Vec::new(),
        },
    )
    .unwrap();
    let reply: ClientReply = read_record(&mut peer).unwrap();
    assert!(matches!(reply, ClientReply::Error { .. }), "{reply:?}");
    write_record(&mut peer, &ClientRequest::Ping).unwrap();
    assert_eq!(
        read_record::<ClientReply>(&mut peer).unwrap(),
        ClientReply::Pong
    );

    drop(peer);
    drop(connection);
    expect_success(&mut child);
}

/// The real Client does not offer managed output: a save `Begin` on the
/// admitted pair is declined with a typed `Unavailable`, the workspace is left
/// exactly as it was, and the same connection keeps answering `Ping`.
#[test]
fn save_is_declined_as_unavailable_without_touching_the_workspace() {
    use rustx_execution_protocol::process::{OperationId, ServerIncarnation};
    use rustx_execution_protocol::save::{
        ContentFormat, ConversationId, SaveCode, SaveOwner, SaveReply, SaveRequest,
    };
    use rustx_execution_protocol::{Generation, SessionId};

    let rig = rig();
    fs::write(rig.workspace_path.join("kept"), "before").unwrap();
    let before = workspace_listing(&rig.workspace_path);

    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let mut peer = stream.try_clone().unwrap();
    let connection = admit_pair_stream(&rig, &mut child, stream);

    let owner = SaveOwner {
        incarnation: ServerIncarnation::generate(),
        session: SessionId::new("session-1"),
        instance: ClientInstanceId::new("instance-1"),
        generation: Generation::new(NonZeroU64::new(1).unwrap()),
        conversation: ConversationId::parse("conv-1").unwrap(),
        operation: OperationId::new(),
    };
    write_record(
        &mut peer,
        &ClientRequest::Save(SaveRequest::Begin {
            owner,
            content_format: ContentFormat::NormalizedUtf8V1,
        }),
    )
    .unwrap();
    let reply: ClientReply = read_record(&mut peer).unwrap();
    assert_eq!(
        reply,
        ClientReply::Save(SaveReply::Unavailable {
            code: SaveCode::CapabilityUnavailable
        })
    );
    assert_eq!(
        workspace_listing(&rig.workspace_path),
        before,
        "a declined save never stages anything in the workspace"
    );
    write_record(&mut peer, &ClientRequest::Ping).unwrap();
    assert_eq!(
        read_record::<ClientReply>(&mut peer).unwrap(),
        ClientReply::Pong
    );

    drop(peer);
    drop(connection);
    expect_success(&mut child);
}

/// A reservation demanding managed output cannot be satisfied by the real
/// Client, which does not advertise that capability: the handshake is rejected
/// for missing capabilities, never admitted.
#[test]
fn a_reservation_requiring_managed_output_is_rejected_by_the_real_client() {
    let rig = rig_with(BTreeSet::from([
        Capability::Connection,
        Capability::ManagedOutput,
    ]));
    let mut child = spawn_client(&rig);
    let stream = accept(&rig, &mut child);
    let outcome = rig.binding.accept_local(stream).unwrap();
    assert!(matches!(
        outcome,
        rustx_execution_server::ChannelAccept::Rejected(Reject::MissingCapabilities)
    ));
    assert!(!child.wait().unwrap().success());
    assert_eq!(rig.binding.status().phase, BindingPhase::Reserved);
}

fn workspace_listing(workspace: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The real Client searches one shared file universe for both `Glob` and
/// `Grep`: the traversal never follows a link outside the binding, keeps
/// hidden files visible, refuses a symlinked root, skips a file whose
/// UTF-8 breaks after an apparent match, and answers a single-file root.
#[test]
fn search_serves_the_shared_universe_and_never_follows_links_outside() {
    let rig = rig();
    // Two links to outside: a directory and a file, neither may be
    // traversed or reported.
    let outside = private_dir().1;
    fs::create_dir_all(outside.join("escape-dir")).unwrap();
    fs::write(outside.join("escape.txt"), "needle\n").unwrap();
    std::os::unix::fs::symlink(
        outside.join("escape-dir"),
        rig.workspace_path.join("linked-dir"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.join("escape.txt"),
        rig.workspace_path.join("linked.txt"),
    )
    .unwrap();
    fs::create_dir_all(rig.workspace_path.join("z")).unwrap();
    fs::write(rig.workspace_path.join("z/deep.txt"), "needle\n").unwrap();
    fs::write(rig.workspace_path.join("a.txt"), "needle\n").unwrap();
    fs::write(rig.workspace_path.join("b.txt"), "quiet\n").unwrap();
    fs::write(rig.workspace_path.join(".hidden.txt"), "needle\n").unwrap();
    // An invalid byte after an apparent match excludes the whole file.
    fs::write(
        rig.workspace_path.join("tail.txt"),
        b"needle\nvalid\n\xff\xfe",
    )
    .unwrap();

    let mut child = spawn_client(&rig);
    let mut connection = admit_pair(&rig, &mut child);

    // Glob: sorted lexical order, hidden files visible, links absent.
    let page = connection
        .search(
            Path::new(""),
            &rustx_execution_protocol::SearchSpec::Glob {
                pattern: "**/*.txt".to_owned(),
                limit: rustx_execution_protocol::DecimalU64::new(100),
            },
        )
        .unwrap();
    assert_eq!(page.body, ".hidden.txt\na.txt\nb.txt\ntail.txt\nz/deep.txt");
    assert_eq!(page.matches, 5);
    assert!(!page.hit_limit && !page.byte_limit && !page.short_lines);

    // Grep: the invalid-UTF8 tail file contributes nothing even though a
    // match precedes the bad bytes; only matching rows cross the wire.
    let page = connection
        .search(
            Path::new(""),
            &rustx_execution_protocol::SearchSpec::Grep {
                pattern: "needle".to_owned(),
                glob: None,
                literal: false,
                ignore_case: false,
                context: 0,
                limit: rustx_execution_protocol::DecimalU64::new(100),
            },
        )
        .unwrap();
    assert_eq!(
        page.body,
        ".hidden.txt:1: needle\na.txt:1: needle\nz/deep.txt:1: needle"
    );
    assert_eq!(page.matches, 3);

    // A single-file root is the whole universe; the match row names the
    // file itself.
    let page = connection
        .search(
            Path::new("a.txt"),
            &rustx_execution_protocol::SearchSpec::Grep {
                pattern: "needle".to_owned(),
                glob: None,
                literal: false,
                ignore_case: false,
                context: 0,
                limit: rustx_execution_protocol::DecimalU64::new(10),
            },
        )
        .unwrap();
    assert_eq!(page.body, "a.txt:1: needle");

    // A symlinked search root is refused, and the connection stays live.
    let error = connection
        .search(
            Path::new("linked-dir"),
            &rustx_execution_protocol::SearchSpec::Glob {
                pattern: "*".to_owned(),
                limit: rustx_execution_protocol::DecimalU64::new(10),
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    connection.ping().unwrap();

    drop(connection);
    expect_success(&mut child);
}
