use std::io::Read;
use std::num::NonZeroU64;
use std::sync::Barrier;
use std::thread;
use std::time::Duration;

use rustx_execution_protocol::EntryKind;
use rustx_execution_protocol::transport::MAX_RECORD_BYTES;

use super::*;

fn required() -> BTreeSet<Capability> {
    BTreeSet::from([Capability::Files, Capability::Process])
}

fn reservation() -> Reservation {
    reservation_in(Path::new("/work/root"))
}

fn reservation_in(workspace: &Path) -> Reservation {
    Reservation {
        session: SessionId::new("session-1"),
        environment: EnvironmentId::new("env-1"),
        instance: ClientInstanceId::new("instance-1"),
        generation: Generation::new(NonZeroU64::MIN),
        workspace: workspace.to_path_buf(),
        required_capabilities: required(),
    }
}

fn valid_hello(credential: &BindingCredential) -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        session: SessionId::new("session-1"),
        environment: EnvironmentId::new("env-1"),
        instance: ClientInstanceId::new("instance-1"),
        generation: Generation::new(NonZeroU64::MIN),
        workspace: PathBuf::from("/work/root"),
        capabilities: required(),
        credential: credential.clone(),
    }
}

#[test]
fn concurrent_hellos_commit_exactly_one_live_connection() {
    let (binding, credential) = Binding::reserve(reservation()).unwrap();
    let hello = valid_hello(&credential);
    let barrier = Barrier::new(2);
    let results: Vec<_> = thread::scope(|scope| {
        let binding = &binding;
        let hello = &hello;
        let barrier = &barrier;
        let handles: Vec<_> = ["conn-a", "conn-b"]
            .into_iter()
            .map(|name| {
                scope.spawn(move || {
                    barrier.wait();
                    binding.connect(hello, ConnectionId::new(name))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == Err(Reject::AlreadyConnected))
            .count(),
        1
    );
    let status = binding.status();
    assert_eq!(status.phase, BindingPhase::Live);
    assert!(matches!(
        status.connection.as_ref().map(ConnectionId::as_str),
        Some("conn-a" | "conn-b")
    ));
}

#[test]
fn wrong_hello_fields_never_consume_a_reservation() {
    type Mutation = Box<dyn Fn(&mut Hello)>;
    let cases: Vec<(&str, Mutation, Reject)> = vec![
        (
            "version",
            Box::new(|h: &mut Hello| h.version = 2),
            Reject::UnsupportedVersion,
        ),
        (
            "session",
            Box::new(|h: &mut Hello| h.session = SessionId::new("other")),
            Reject::SessionMismatch,
        ),
        (
            "environment",
            Box::new(|h: &mut Hello| {
                h.environment = EnvironmentId::new("other");
            }),
            Reject::EnvironmentMismatch,
        ),
        (
            "instance",
            Box::new(|h: &mut Hello| {
                h.instance = ClientInstanceId::new("other");
            }),
            Reject::InstanceMismatch,
        ),
        (
            "generation",
            Box::new(|h: &mut Hello| {
                h.generation = Generation::new(NonZeroU64::new(2).unwrap());
            }),
            Reject::GenerationMismatch,
        ),
        (
            "workspace",
            Box::new(|h: &mut Hello| {
                h.workspace = PathBuf::from("/work/other");
            }),
            Reject::WorkspaceMismatch,
        ),
        // `Path` equality would normalize `//` away and admit this;
        // the literal OsStr comparison must not.
        (
            "workspace separator alias",
            Box::new(|h: &mut Hello| {
                h.workspace = PathBuf::from("/work//root");
            }),
            Reject::WorkspaceMismatch,
        ),
        (
            "workspace dot alias",
            Box::new(|h: &mut Hello| {
                h.workspace = PathBuf::from("/work/./root");
            }),
            Reject::WorkspaceMismatch,
        ),
        (
            "required capability",
            Box::new(|h: &mut Hello| {
                h.capabilities = BTreeSet::from([Capability::Files]);
            }),
            Reject::MissingCapabilities,
        ),
        (
            "credential",
            Box::new(|h: &mut Hello| {
                h.credential = BindingCredential::from_bytes([0xa5; 32]);
            }),
            Reject::InvalidCredential,
        ),
    ];

    for (name, mutate, expected) in cases {
        let (binding, credential) = Binding::reserve(reservation()).unwrap();
        let mut wrong = valid_hello(&credential);
        mutate(&mut wrong);
        assert_eq!(
            binding.connect(&wrong, ConnectionId::new("conn-1")),
            Err(expected),
            "{name}"
        );
        assert_eq!(binding.status().phase, BindingPhase::Reserved, "{name}");
        // The failed Hello consumed nothing: a correct Hello still wins.
        assert!(
            binding
                .connect(&valid_hello(&credential), ConnectionId::new("conn-1"))
                .is_ok()
        );
        assert_eq!(binding.status().phase, BindingPhase::Live, "{name}");
    }
}

#[test]
fn reject_after_live_does_not_displace_the_live_connection() {
    let (binding, credential) = Binding::reserve(reservation()).unwrap();
    let hello = valid_hello(&credential);
    binding
        .connect(&hello, ConnectionId::new("conn-1"))
        .unwrap();

    let mut wrong_instance = hello.clone();
    wrong_instance.instance = ClientInstanceId::new("other");
    assert_eq!(
        binding.connect(&wrong_instance, ConnectionId::new("conn-2")),
        Err(Reject::InstanceMismatch)
    );
    let status = binding.status();
    assert_eq!(status.phase, BindingPhase::Live);
    assert_eq!(status.connection.unwrap().as_str(), "conn-1");
}

#[test]
fn revoke_is_immediate_and_terminal() {
    let (binding, credential) = Binding::reserve(reservation()).unwrap();
    let hello = valid_hello(&credential);

    binding.revoke();
    assert_eq!(
        binding.connect(&hello, ConnectionId::new("conn-1")),
        Err(Reject::Revoked)
    );
    assert_eq!(binding.status().phase, BindingPhase::Revoked);

    let (binding, credential) = Binding::reserve(reservation()).unwrap();
    let valid = valid_hello(&credential);
    binding
        .connect(&valid, ConnectionId::new("conn-1"))
        .unwrap();
    binding.revoke();
    let status = binding.status();
    assert_eq!(status.phase, BindingPhase::Revoked);
    assert_eq!(status.connection, None);
    assert_eq!(
        binding.connect(&valid, ConnectionId::new("conn-2")),
        Err(Reject::Revoked)
    );
}

#[test]
fn stale_disconnect_is_ignored_and_detach_never_reopens_the_reservation() {
    let (binding, credential) = Binding::reserve(reservation()).unwrap();
    let hello = valid_hello(&credential);
    binding
        .connect(&hello, ConnectionId::new("conn-1"))
        .unwrap();

    // A foreign or stale disconnect cannot tear down the live connection.
    binding.disconnect(&ConnectionId::new("stale"));
    assert_eq!(binding.status().connection.unwrap().as_str(), "conn-1");

    // The real EOF detaches; the reservation does not reopen.
    binding.disconnect(&ConnectionId::new("conn-1"));
    let status = binding.status();
    assert_eq!(status.phase, BindingPhase::Detached);
    assert_eq!(status.connection, None);

    // A different instance cannot take over a detached binding.
    let mut replacement = hello.clone();
    replacement.instance = ClientInstanceId::new("instance-2");
    assert_eq!(
        binding.connect(&replacement, ConnectionId::new("conn-2")),
        Err(Reject::InstanceMismatch)
    );
    assert_eq!(binding.status().phase, BindingPhase::Detached);

    // Even the fully valid reserved identity cannot replay its own
    // credential after EOF: re-admission needs recovery proof (#577).
    assert_eq!(
        binding.connect(&hello, ConnectionId::new("conn-2")),
        Err(Reject::RecoveryRequired)
    );
    assert_eq!(binding.status().phase, BindingPhase::Detached);
}

#[test]
fn credential_is_verified_before_any_field_probe() {
    let (binding, credential) = Binding::reserve(reservation()).unwrap();
    // Every field wrong, credential wrong: the verifier runs first.
    let mut probe = valid_hello(&credential);
    probe.credential = BindingCredential::from_bytes([0x00; 32]);
    probe.session = SessionId::new("other");
    probe.version = 2;
    assert_eq!(
        binding.connect(&probe, ConnectionId::new("conn-1")),
        Err(Reject::InvalidCredential)
    );
    // Every field wrong, credential right: field checks then report.
    let mut probe = valid_hello(&credential);
    probe.session = SessionId::new("other");
    probe.environment = EnvironmentId::new("other");
    assert_eq!(
        binding.connect(&probe, ConnectionId::new("conn-1")),
        Err(Reject::SessionMismatch)
    );
    assert_eq!(binding.status().phase, BindingPhase::Reserved);
}

#[test]
fn reserve_rejects_unnormalized_or_relative_workspaces() {
    for workspace in ["work/root", "/work//root", "/work/../root", "/work/"] {
        let mut reservation = reservation();
        reservation.workspace = PathBuf::from(workspace);
        assert!(Binding::reserve(reservation).is_err(), "{workspace}");
    }
}

#[test]
fn reserve_local_requires_private_parent_and_leaves_no_leftovers() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = tempfile::tempdir().unwrap();
    let public = tempfile::tempdir().unwrap();
    std::fs::set_permissions(public.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

    // A world-readable parent is never trusted, even for a valid child.
    assert!(
        Binding::reserve_local(
            reservation_in(workspace.path()),
            &public.path().join("client.sock"),
            &public.path().join("bootstrap"),
        )
        .is_err()
    );
    assert!(!public.path().join("client.sock").exists());

    // A preexisting socket path is never silently unlinked.
    let private = tempfile::tempdir().unwrap();
    std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = private.path().join("client.sock");
    let bundle = private.path().join("bootstrap");
    std::fs::write(&socket, b"occupied").unwrap();
    assert!(Binding::reserve_local(reservation_in(workspace.path()), &socket, &bundle).is_err());
    assert_eq!(std::fs::read(&socket).unwrap(), b"occupied");

    // A preexisting bootstrap path fails publication and removes the
    // freshly bound socket: no leftovers.
    std::fs::remove_file(&socket).unwrap();
    std::fs::write(&bundle, b"occupied").unwrap();
    assert!(Binding::reserve_local(reservation_in(workspace.path()), &socket, &bundle).is_err());
    assert!(!socket.exists());
    assert_eq!(std::fs::read(&bundle).unwrap(), b"occupied");
}

/// Reserves a real binding inside a private `0700` directory and
/// returns its retained published-bundle proof for tamper tests.
fn reserved_bundle(workspace: &Path) -> (tempfile::TempDir, std::path::PathBuf, PublishedBundle) {
    use std::os::unix::fs::PermissionsExt;
    let private = tempfile::tempdir().unwrap();
    std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = private.path().join("s");
    let bundle = private.path().join("b");
    let (_binding, _listener, published) =
        Binding::reserve_local(reservation_in(workspace), &socket, &bundle).unwrap();
    (private, bundle, published)
}

#[test]
fn published_bundle_remove_preserves_foreign_replacements() {
    use std::os::unix::fs::FileTypeExt;

    let workspace = tempfile::tempdir().unwrap();
    let (private, bundle, published) = reserved_bundle(workspace.path());
    let original = std::fs::read(&bundle).unwrap();
    assert!(
        std::fs::symlink_metadata(&bundle)
            .unwrap()
            .file_type()
            .is_file()
    );

    // Identical bytes on a different inode: the foreign file is
    // never ours, so it is preserved byte-for-byte.
    std::fs::remove_file(&bundle).unwrap();
    std::fs::write(&bundle, &original).unwrap();
    published.remove();
    assert_eq!(std::fs::read(&bundle).unwrap(), original);

    // A FIFO replacement is never opened — removal is prompt and
    // the FIFO survives untouched.
    std::fs::remove_file(&bundle).unwrap();
    nix::unistd::mkfifo(
        &bundle,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    published.remove();
    assert!(
        std::fs::symlink_metadata(&bundle)
            .unwrap()
            .file_type()
            .is_fifo()
    );

    // A symlink replacement — even one pointing at a real user file
    // inside the same directory — preserves both link and target.
    std::fs::remove_file(&bundle).unwrap();
    let target = private.path().join("target");
    std::fs::write(&target, b"user data").unwrap();
    std::os::unix::fs::symlink(&target, &bundle).unwrap();
    published.remove();
    assert!(
        std::fs::symlink_metadata(&bundle)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"user data");
    std::fs::remove_file(&bundle).unwrap();
    std::fs::remove_file(&target).unwrap();

    // And while our own file still occupies the path, removal does
    // unlink exactly it.
    std::fs::write(&bundle, &original).unwrap();
    let (private2, bundle2, published2) = reserved_bundle(workspace.path());
    let _ = private;
    published2.remove();
    assert!(!bundle2.exists());
    assert!(private2.path().join("s").exists());
}

#[test]
fn published_bundle_remove_survives_ancestor_rename() {
    let workspace = tempfile::tempdir().unwrap();
    let (private, bundle, published) = reserved_bundle(workspace.path());

    // Renaming the parent away moves the real directory — and the
    // retained descriptor still tracks it, so our file inside is
    // removed while a foreign replacement at the original path is
    // never even seen.
    let destination = tempfile::tempdir().unwrap();
    let moved = destination.path().join("moved");
    std::fs::rename(private.path(), &moved).unwrap();
    std::fs::create_dir(private.path()).unwrap();
    std::fs::write(private.path().join("b"), b"foreign").unwrap();

    published.remove();
    assert!(!moved.join("b").exists(), "our file was removed");
    assert_eq!(
        std::fs::read(private.path().join("b")).unwrap(),
        b"foreign",
        "the foreign replacement is preserved"
    );
    let _ = bundle;
}

#[test]
fn extra_advertised_capabilities_do_not_relax_requirements() {
    let (binding, credential) = Binding::reserve(reservation()).unwrap();
    let mut hello = valid_hello(&credential);
    hello.capabilities = BTreeSet::from([
        Capability::Files,
        Capability::Process,
        Capability::BoundedSearch,
    ]);
    assert!(binding.connect(&hello, ConnectionId::new("conn-1")).is_ok());
}

fn reserved(capabilities: BTreeSet<Capability>) -> (Binding, Hello) {
    let mut reserved = reservation();
    reserved.required_capabilities = capabilities.clone();
    let (binding, credential) = Binding::reserve(reserved).unwrap();
    let mut hello = valid_hello(&credential);
    hello.capabilities = capabilities;
    (binding, hello)
}

/// The client-side `ControlHello` a paired lane carries — the data
/// Hello's exact identity plus the challenge the Server issued.
fn control_hello(hello: &Hello, challenge: PairingNonce) -> PeerHello {
    PeerHello::Control(ControlHello {
        version: hello.version,
        session: hello.session.clone(),
        environment: hello.environment.clone(),
        instance: hello.instance.clone(),
        generation: hello.generation,
        workspace: hello.workspace.clone(),
        credential: hello.credential.clone(),
        connection_nonce: challenge,
    })
}

/// The minimal client-side control loop the tests' paired lanes run:
/// every `ControlPoll` carries a fresh deterministic nonce and the
/// receipt for the previous request; `Health` echoes, `Revoke`
/// acknowledges — `CancelMutation` answers `Unknown` because these
/// peers stage no local mutations. The loop ends when the lane does.
fn control_peer_loop(channel: &mut rustx_execution_transport::RecordChannel) {
    let mut sequence = 1_u64;
    let mut receipt: Option<ControlReply> = None;
    loop {
        let deadline = Instant::now() + Duration::from_secs(5);
        let poll = ControlPoll {
            sequence: DecimalU64::new(sequence),
            receipt: receipt.take(),
            health: DecimalU64::new(sequence),
        };
        if channel
            .send(&encode_record(&poll).unwrap(), Some(deadline))
            .is_err()
        {
            return;
        }
        let Ok(turn) = channel.receive::<ControlTurn>(Some(deadline)) else {
            return;
        };
        if turn.sequence != poll.sequence || turn.health != poll.health {
            return;
        }
        sequence += 1;
        let Some(request) = turn.request else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        let done = matches!(request, ControlRequest::Revoke);
        receipt = Some(match request {
            ControlRequest::Health { nonce } => ControlReply::Healthy { nonce },
            ControlRequest::CancelMutation { ticket } => ControlReply::MutationCancelled {
                ticket,
                disposition: CancelDisposition::Unknown,
            },
            ControlRequest::Revoke => ControlReply::Revoked,
            ControlRequest::StopWatch { id } => ControlReply::WatchStopped { id, settled: true },
            ControlRequest::CancelProcess { key } => ControlReply::ProcessCancelled {
                key,
                disposition: rustx_execution_protocol::ProcessCancelDisposition::Unknown,
            },
        });
        if done {
            let poll = ControlPoll {
                sequence: DecimalU64::new(sequence),
                receipt,
                health: DecimalU64::new(sequence),
            };
            let _ = channel.send(
                &encode_record(&poll).unwrap(),
                Some(Instant::now() + Duration::from_secs(5)),
            );
            return;
        }
    }
}

/// One real loopback WebSocket pair admitted through
/// `accept_channel_deadline` — the genuine RFC 6455 upgrade on both
/// client lanes, never a UDS stand-in. Returns the live connection
/// and the data peer channel for driving typed replies; the control
/// peer polls on its own bounded thread.
fn ws_connection(
    binding: &Binding,
    hello: &Hello,
) -> (LocalConnection, rustx_execution_transport::RecordChannel) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let endpoint = || {
        rustx_execution_transport::ExecutionUrl::validate(&format!("ws://{address}/execution/v1"))
            .unwrap()
    };
    // The data half: `PeerHello::Data` earns `Pending` and the
    // pairing challenge.
    let (challenge, client) = thread::scope(|scope| {
        let server = scope.spawn(|| {
            let (stream, _) = listener.accept().unwrap();
            let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
            binding.accept_channel_deadline(channel, deadline).unwrap()
        });
        let mut client = rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
        client
            .send(
                &encode_record(&PeerHello::Data(hello.clone())).unwrap(),
                Some(deadline),
            )
            .unwrap();
        let reply: DataHelloReply = client.receive(Some(deadline)).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };
        assert!(matches!(server.join().unwrap(), ChannelAccept::Pending));
        (data.challenge, client)
    });
    // The pairing control lane: a second real WebSocket connection
    // carrying the challenge commits the pair; its peer keeps
    // polling on a bounded thread.
    let connection = thread::scope(|scope| {
        let server = scope.spawn(|| {
            let (stream, _) = listener.accept().unwrap();
            let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
            binding.accept_channel_deadline(channel, deadline).unwrap()
        });
        let mut control = rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
        control
            .send(
                &encode_record(&control_hello(hello, challenge)).unwrap(),
                Some(deadline),
            )
            .unwrap();
        let reply: ControlHelloReply = control.receive(Some(deadline)).unwrap();
        assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
        let ChannelAccept::Live(connection) = server.join().unwrap() else {
            panic!("the paired control lane commits the connection");
        };
        thread::spawn(move || control_peer_loop(&mut control));
        connection
    });
    (connection, client)
}

#[test]
fn a_failed_local_preencode_never_retires_a_websocket_connection() {
    let (binding, hello) = reserved(required());
    let (mut connection, mut client) = ws_connection(&binding, &hello);
    // A request that cannot be encoded fails locally before the gate
    // and before a single byte reaches the wire — the channel is
    // never retired for it.
    let oversized = ClientRequest::WriteChunk {
        ticket: DecimalU64::new(1),
        offset: DecimalU64::new(0),
        data_base64: "a".repeat(MAX_RECORD_BYTES + 1),
    };
    assert!(connection.exchange(&oversized).is_err());
    let deadline = Instant::now() + Duration::from_secs(10);
    let pong = thread::spawn(move || {
        let request: ClientRequest = client.receive(Some(deadline)).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        client
            .send(&encode_record(&ClientReply::Pong).unwrap(), Some(deadline))
            .unwrap();
    });
    connection.ping().unwrap();
    pong.join().unwrap();
}

#[test]
fn websocket_exchange_carries_identical_typed_records() {
    let (binding, hello) = reserved(required());
    let (mut connection, mut client) = ws_connection(&binding, &hello);
    let deadline = Instant::now() + Duration::from_secs(10);
    let answer = thread::spawn(move || {
        let request: ClientRequest = client.receive(Some(deadline)).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        client
            .send(&encode_record(&ClientReply::Pong).unwrap(), Some(deadline))
            .unwrap();
    });
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn websocket_revocation_wakes_a_blocked_exchange() {
    let (binding, hello) = reserved(required());
    let (mut connection, _client) = ws_connection(&binding, &hello);
    // The peer never replies: the exchange blocks until revoke shuts
    // the raw TCP handle the binding committed at admission.
    let exchange = thread::spawn(move || {
        connection
            .read_range(Path::new("file"), 0, 64, None)
            .unwrap_err()
    });
    thread::sleep(Duration::from_millis(100));
    binding.revoke();
    let error = exchange.join().unwrap();
    assert!(
        !matches!(error.kind(), io::ErrorKind::TimedOut),
        "revocation woke the blocked exchange: {error:?}"
    );
    assert_eq!(binding.status().phase, BindingPhase::Revoked);
}

#[test]
fn a_peer_without_the_range_capability_gets_no_request() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // No admitted capability: the call fails locally and no request is
    // ever placed on the wire.
    let error = connection
        .read_range(Path::new("file"), 0, 64, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    peer.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert!(read_record::<ClientRequest>(&mut peer).is_err());
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

    // The connection still serves the typed liveness exchange.
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

/// One fully admitted live pair over real socket pairs: the data
/// Hello is written by the peer, its challenge pairs a second lane
/// whose peer keeps polling on a bounded thread, and the data peer
/// end comes back for scripted replies.
fn live(binding: &Binding, hello: &Hello) -> (LocalConnection, UnixStream) {
    let (mut peer, stream) = UnixStream::pair().unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    (live_data_half(binding, hello, stream, &mut peer), peer)
}

/// The paired admission on an already-configured server stream: the
/// data half must already carry `PeerHello::Data(hello)` — this
/// admits it `Pending`, pairs the mandatory control lane on a fresh
/// socket pair, and returns the committed connection. The control
/// peer polls on its own bounded thread.
fn live_data_half(
    binding: &Binding,
    hello: &Hello,
    stream: UnixStream,
    peer: &mut UnixStream,
) -> LocalConnection {
    assert!(matches!(
        binding.accept_local(stream).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(peer).unwrap();
    let Ok(data) = reply else {
        panic!("the honest data Hello is admitted");
    };
    pair_control_half(binding, hello, &data.challenge)
}

/// Commits a pending data half: a second socket pair carries the
/// `ControlHello` challenge, the committed connection comes back,
/// and the scripted control peer keeps polling so the pair stays
/// live for the test's lifetime.
fn pair_control_half(
    binding: &Binding,
    hello: &Hello,
    challenge: &PairingNonce,
) -> LocalConnection {
    let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
    write_record(&mut control_peer, &control_hello(hello, challenge.clone())).unwrap();
    let accepted = binding.accept_local(control_stream).unwrap();
    let ChannelAccept::Live(connection) = accepted else {
        panic!("the paired control lane commits the connection");
    };
    let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
    assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
    thread::spawn(move || {
        let mut channel = RecordChannel::unix(control_peer);
        control_peer_loop(&mut channel);
    });
    connection
}

#[test]
fn the_physical_binding_fences_a_delegated_commit_receipt_with_the_wrong_size() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalWrite,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);
    let ticket = DecimalU64::new(3);
    let worker = thread::spawn(move || {
        assert_eq!(
            read_record::<ClientRequest>(&mut peer).unwrap(),
            ClientRequest::CommitWrite { ticket }
        );
        write_record(
            &mut peer,
            &ClientReply::Written {
                ticket,
                metadata: FileStamp {
                    size: DecimalU64::new(99),
                    revision: "a".repeat(64),
                    identity: "1:1".into(),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let receipt = connection
        .forward_delegated(
            &ClientRequest::CommitWrite { ticket },
            deadline,
            Some(3),
            || Ok(()),
        )
        .unwrap();
    assert_eq!(receipt, ClientReply::CommitUnknown { ticket });
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    worker.join().unwrap();
}

#[test]
fn disappearance_after_the_first_range_is_a_conflict_and_keeps_the_pair_live() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ReadRange,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);
    let worker = thread::spawn(move || {
        let path = PathBuf::from("rustx.toml");
        let revision = "a".repeat(64);
        let chunk = MAX_RAW_CHUNK_BYTES as u64;
        let bound = u32::try_from(chunk).unwrap();
        assert_eq!(
            read_record::<ClientRequest>(&mut peer).unwrap(),
            ClientRequest::ReadRange {
                relative_path: path.clone(),
                offset: DecimalU64::new(0),
                max_bytes: bound,
                expected_revision: None,
            }
        );
        write_record(
            &mut peer,
            &ClientReply::Range {
                data_base64: base64::engine::general_purpose::STANDARD
                    .encode(vec![b'a'; MAX_RAW_CHUNK_BYTES]),
                offset: DecimalU64::new(0),
                next_offset: Some(DecimalU64::new(chunk)),
                metadata: FileStamp {
                    size: DecimalU64::new(chunk + 1),
                    revision: revision.clone(),
                    identity: "1:1".into(),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
        assert_eq!(
            read_record::<ClientRequest>(&mut peer).unwrap(),
            ClientRequest::ReadRange {
                relative_path: path.clone(),
                offset: DecimalU64::new(chunk),
                max_bytes: bound,
                expected_revision: Some(revision),
            }
        );
        write_record(
            &mut peer,
            &ClientReply::Error {
                kind: ReadError::NotFound,
            },
        )
        .unwrap();
        assert_eq!(
            read_record::<ClientRequest>(&mut peer).unwrap(),
            ClientRequest::ReadRange {
                relative_path: path,
                offset: DecimalU64::new(0),
                max_bytes: bound,
                expected_revision: None,
            }
        );
        write_record(
            &mut peer,
            &ClientReply::Error {
                kind: ReadError::NotFound,
            },
        )
        .unwrap();
        assert_eq!(
            read_record::<ClientRequest>(&mut peer).unwrap(),
            ClientRequest::Ping
        );
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let limit = MAX_RAW_CHUNK_BYTES as u64 + 1;
    let first = connection
        .read_bounded_file(Path::new("rustx.toml"), limit, deadline, || Ok(()))
        .unwrap_err();
    assert_eq!(first.kind(), io::ErrorKind::AlreadyExists);
    let second = connection
        .read_bounded_file(Path::new("rustx.toml"), limit, deadline, || Ok(()))
        .unwrap_err();
    assert_eq!(second.kind(), io::ErrorKind::NotFound);
    connection.ping().unwrap();
    worker.join().unwrap();
}

#[test]
fn an_unencodable_request_never_reaches_the_wire() {
    use std::os::unix::ffi::OsStrExt;

    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ReadRange,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // A relative path serde cannot encode and a bound-breaking
    // expected revision both fail at `encode_record`, before the
    // gate: the request never touches the wire.
    let error = connection
        .read_range(
            Path::new(std::ffi::OsStr::from_bytes(b"name\xff")),
            0,
            64,
            None,
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    let oversized = "a".repeat(MAX_RECORD_BYTES);
    let error = connection
        .read_range(Path::new("file"), 0, 64, Some(&oversized))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);

    // Prove nothing was written: a nonblocking probe reports
    // WouldBlock, then blocking I/O resumes.
    peer.set_nonblocking(true).unwrap();
    let mut probe = [0u8; 1];
    assert_eq!(
        peer.read(&mut probe).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    peer.set_nonblocking(false).unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);

    // The still-live connection answers an exact Ping and then a
    // valid read.
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadRange { offset, .. } = request else {
            panic!("expected a range request");
        };
        write_record(
            &mut peer,
            &ClientReply::Range {
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"ab"),
                offset,
                next_offset: None,
                metadata: FileStamp {
                    size: DecimalU64::new(2),
                    revision: "a".repeat(64),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
    });
    connection.ping().unwrap();
    let range = connection
        .read_range(Path::new("file"), 0, 64, None)
        .unwrap();
    assert_eq!(range.bytes, b"ab");
    answer.join().unwrap();
}

#[test]
fn a_peer_closing_mid_request_detaches_the_binding() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ReadRange,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // The peer reads the request's prefix and then shuts down: the
    // reply accounting can never be trusted again.
    let closer = thread::spawn(move || {
        let mut prefix = [0u8; 4];
        peer.read_exact(&mut prefix).unwrap();
        peer.shutdown(std::net::Shutdown::Both).unwrap();
    });
    let error = connection
        .read_range(Path::new("file"), 0, 64, None)
        .unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
    ));
    closer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn an_expired_deadline_before_commit_keeps_the_reservation() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));

    // A deadline already expired at accept: no Hello is ever read.
    let (_peer, stream) = UnixStream::pair().unwrap();
    let Err(error) = binding.accept_local_deadline(stream, Instant::now()) else {
        panic!("an expired deadline must fail before the Hello");
    };
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert_eq!(binding.status().phase, BindingPhase::Reserved);

    // The honest Hello still wins the live connection afterwards.
    assert!(binding.connect(&hello, ConnectionId::new("conn-2")).is_ok());
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn pairing_commit_refuses_data_eof_before_the_watchdog_can_run() {
    let (binding, hello) = reserved(required());
    let (data_peer, data_stream) = UnixStream::pair().unwrap();
    let deadline = Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
    let (challenge, attempt, _) = binding
        .allocate_pending(&hello, ConnectionId::new("pending"), deadline)
        .unwrap()
        .unwrap();
    let channel = RecordChannel::unix(data_stream);
    let shutdown = channel.shutdown_handle().unwrap();
    // Store the exact pending lane without scheduling its asynchronous
    // watchdog. The control commit must discover the EOF itself.
    assert!(binding.store_pending_channel(attempt, channel, shutdown));
    drop(data_peer);
    let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
    write_record(&mut control_peer, &control_hello(&hello, challenge)).unwrap();
    assert!(matches!(
        binding
            .accept_local_deadline(control_stream, deadline)
            .unwrap(),
        ChannelAccept::Rejected(Reject::InvalidPairing)
    ));
    assert_eq!(
        read_record::<ControlHelloReply>(&mut control_peer).unwrap(),
        Err(Reject::InvalidPairing)
    );
    assert_eq!(binding.status().phase, BindingPhase::Reserved);
    assert!(!binding.status().pending);
    let (_new, _data) = live(&binding, &hello);
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_closed_data_half_before_pairing_discards_the_attempt() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
    let (mut peer, stream) = UnixStream::pair().unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(stream).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer).unwrap();
    let Ok(data) = reply else {
        panic!("the honest data Hello is admitted");
    };
    assert_eq!(binding.status().phase, BindingPhase::Reserved);

    // The data half dies before the control lane pairs: the watchdog
    // sees the lane's end and discards the attempt — the reservation
    // stays Reserved and the now-stale nonce pairs nothing.
    drop(peer);
    let deadline = Instant::now() + Duration::from_secs(10);
    while binding.status().pending {
        assert!(
            Instant::now() < deadline,
            "the watchdog never discarded the attempt"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
    write_record(
        &mut control_peer,
        &control_hello(&hello, data.challenge.clone()),
    )
    .unwrap();
    assert!(matches!(
        binding.accept_local(control_stream).unwrap(),
        ChannelAccept::Rejected(Reject::InvalidPairing)
    ));
    let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
    assert!(matches!(reply, Err(Reject::InvalidPairing)));
    assert_eq!(binding.status().phase, BindingPhase::Reserved);
}

#[test]
fn an_invalid_revision_retires_the_stream_and_detaches_the_binding() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ReadRange,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadRange { offset, .. } = request else {
            panic!("expected a range request");
        };
        write_record(
            &mut peer,
            &ClientReply::Range {
                data_base64: String::new(),
                offset,
                next_offset: None,
                metadata: FileStamp {
                    size: DecimalU64::new(0),
                    revision: String::from("not-a-revision"),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
    });
    let error = connection
        .read_range(Path::new("file"), 0, 64, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();

    // The violation detached the binding and retired the stream:
    // nothing further is ever requested on it.
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

fn process_key() -> rustx_execution_protocol::process::ProcessOperationKey {
    use rustx_execution_protocol::process::{OperationId, ServerIncarnation};
    rustx_execution_protocol::process::ProcessOperationKey {
        incarnation: ServerIncarnation::generate(),
        session: rustx_execution_protocol::SessionId::new("session-1"),
        instance: rustx_execution_protocol::ClientInstanceId::new("client-1"),
        generation: rustx_execution_protocol::Generation::try_from(1u64).unwrap(),
        operation: OperationId::new(),
    }
}

#[test]
fn a_contradictory_process_reply_retires_the_pair() {
    use rustx_execution_protocol::DecimalU64;
    use rustx_execution_protocol::process::{ProcessReply, ProcessRequest};

    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::Process,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);
    let key = process_key();
    let request = ProcessRequest::Start { key: key.clone() };

    let answer = thread::spawn(move || {
        let received: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(received, ClientRequest::Process(_)));
        write_record(
            &mut peer,
            &ClientReply::Process(ProcessReply::InputAccepted {
                key,
                sequence: DecimalU64::new(1),
            }),
        )
        .unwrap();
    });
    let error = connection.process(&request).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();

    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_prewire_invalid_output_bound_leaves_the_pair_healthy() {
    use rustx_execution_protocol::DecimalU64;
    use rustx_execution_protocol::process::ProcessRequest;

    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::Process,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);
    let request = ProcessRequest::OutputPage {
        key: process_key(),
        cursor: DecimalU64::new(0),
        max_bytes: 0,
    };
    let error = connection.process(&request).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_size_contradiction_retires_the_stream_and_detaches_the_binding() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ReadRange,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // The peer declares premature EOF: bytes end below the stamped size
    // yet no continuation cursor is offered and the bound is unfilled.
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadRange { offset, .. } = request else {
            panic!("expected a range request");
        };
        write_record(
            &mut peer,
            &ClientReply::Range {
                data_base64: base64::engine::general_purpose::STANDARD.encode(b"ab"),
                offset,
                next_offset: None,
                metadata: FileStamp {
                    size: DecimalU64::new(5),
                    revision: "a".repeat(64),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
    });
    let error = connection
        .read_range(Path::new("file"), 0, 64, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();

    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

fn entry(name: &str) -> PageEntry {
    PageEntry {
        name: name.into(),
        metadata: EntryStamp {
            kind: EntryKind::RegularFile,
            stamp: FileStamp {
                size: DecimalU64::new(1),
                revision: "a".repeat(64),
                identity: String::from("1:1"),
                modified_at_ms: None,
            },
        },
    }
}

#[test]
fn a_missing_directory_capability_denies_before_the_wire() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ReadRange,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    for error in [
        connection.entry_metadata(Path::new("file")).unwrap_err(),
        connection
            .directory_page(Path::new(""), 0, 16, None)
            .unwrap_err(),
    ] {
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    // Nothing was written; the connection stays live and still
    // answers an exact Ping.
    peer.set_nonblocking(true).unwrap();
    let mut byte = [0u8; 1];
    assert_eq!(
        peer.read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    peer.set_nonblocking(false).unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    connection.ping().unwrap();
    answer.join().unwrap();
}

#[test]
fn an_out_of_order_directory_reply_retires_the_stream() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DirectoryMetadata,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // Names that are not strictly increasing are untrusted wire
    // data: the stream retires and the binding detaches.
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDirectory { .. } = request else {
            panic!("expected a directory request");
        };
        write_record(
            &mut peer,
            &ClientReply::Directory {
                revision: "a".repeat(64),
                entries: vec![entry("b"), entry("a")],
                next_offset: None,
            },
        )
        .unwrap();
    });
    let error = connection
        .directory_page(Path::new(""), 0, 16, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();

    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_directory_reply_with_a_wrong_cursor_retires_the_stream() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DirectoryMetadata,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // A full page whose continuation cursor does not equal
    // offset + entries.len() violates the paging contract.
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDirectory { offset, .. } = request else {
            panic!("expected a directory request");
        };
        write_record(
            &mut peer,
            &ClientReply::Directory {
                revision: "a".repeat(64),
                entries: vec![entry("a"), entry("b")],
                next_offset: Some(offset + 99),
            },
        )
        .unwrap();
    });
    let error = connection
        .directory_page(Path::new(""), 0, 16, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();

    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_nul_directory_name_retires_the_stream() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DirectoryMetadata,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // Every other control byte is literal name data; NUL is the one
    // byte no path API could ever carry.
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDirectory { .. } = request else {
            panic!("expected a directory request");
        };
        write_record(
            &mut peer,
            &ClientReply::Directory {
                revision: "a".repeat(64),
                entries: vec![entry("a\0b"), entry("a\u{1f}")],
                next_offset: None,
            },
        )
        .unwrap();
    });
    let error = connection
        .directory_page(Path::new(""), 0, 16, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();

    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_directory_reply_past_the_name_cap_retires_the_stream() {
    // A continuation cursor may never sit at the cap itself: a
    // resumption there could never deliver another entry, so the
    // `Some` is a lie even when the arithmetic agrees.
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DirectoryMetadata,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    let cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap();
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDirectory { offset, .. } = request else {
            panic!("expected a directory request");
        };
        assert_eq!(offset, cap - 1);
        write_record(
            &mut peer,
            &ClientReply::Directory {
                revision: "a".repeat(64),
                entries: vec![entry("a")],
                next_offset: Some(cap),
            },
        )
        .unwrap();
    });
    let error = connection
        .directory_page(Path::new(""), cap - 1, 1, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());

    // And a reply claiming entries past the cap is corrupt even on
    // a final page — the delivered names themselves exceed the
    // physical listing bound, `next=None` notwithstanding.
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DirectoryMetadata,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDirectory { offset, .. } = request else {
            panic!("expected a directory request");
        };
        assert_eq!(offset, cap);
        write_record(
            &mut peer,
            &ClientReply::Directory {
                revision: "a".repeat(64),
                entries: vec![entry("a")],
                next_offset: None,
            },
        )
        .unwrap();
    });
    let error = connection
        .directory_page(Path::new(""), cap, 1, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_directory_page_ending_exactly_at_the_name_cap_is_legitimate() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DirectoryMetadata,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    // The final entry may land exactly on the cap: `next=None` is
    // the honest answer — the directory can hold no more.
    let cap = u32::try_from(MAX_DIRECTORY_NAMES).unwrap();
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDirectory { offset, .. } = request else {
            panic!("expected a directory request");
        };
        assert_eq!(offset, cap - 2);
        write_record(
            &mut peer,
            &ClientReply::Directory {
                revision: "a".repeat(64),
                entries: vec![entry("a"), entry("b")],
                next_offset: None,
            },
        )
        .unwrap();
    });
    let page = connection
        .directory_page(Path::new(""), cap - 2, 16, None)
        .unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.next_offset, None);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_dot_segment_directory_name_retires_the_stream() {
    let (binding, hello) = reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DirectoryMetadata,
    ]));
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDirectory { .. } = request else {
            panic!("expected a directory request");
        };
        write_record(
            &mut peer,
            &ClientReply::Directory {
                revision: "a".repeat(64),
                entries: vec![entry("..")],
                next_offset: None,
            },
        )
        .unwrap();
    });
    let error = connection
        .directory_page(Path::new(""), 0, 16, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();

    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

fn write_stamp(revision: &str) -> FileStamp {
    FileStamp {
        size: DecimalU64::new(4),
        revision: revision.to_owned(),
        identity: String::from("1:1"),
        modified_at_ms: None,
    }
}

fn write_rig() -> (Binding, Hello) {
    reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalWrite,
    ]))
}

fn edit_rig() -> (Binding, Hello) {
    reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ConditionalEdit,
    ]))
}

fn replacement(old_text: &str, new_text: &str) -> TextReplacement {
    TextReplacement {
        old_text: old_text.to_owned(),
        new_text: new_text.to_owned(),
    }
}

fn assert_outcome_unknown(error: &io::Error) {
    let unknown = match error.get_ref() {
        Some(source) => source.is::<WriteOutcomeUnknown>(),
        None => false,
    };
    assert!(unknown, "{error:?}");
}

#[test]
fn a_peer_without_the_write_capability_gets_no_request() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
    let (mut connection, mut peer) = live(&binding, &hello);

    let error = connection
        .write_file(Path::new("file"), b"bytes", None, |_| Ok(()))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    peer.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert!(read_record::<ClientRequest>(&mut peer).is_err());
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_conditional_write_prepares_authorizes_and_commits_on_one_connection() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::PrepareWrite {
            relative_path,
            expectation,
            data_base64,
        } = request
        else {
            panic!("expected a prepare request");
        };
        assert_eq!(relative_path, PathBuf::from("file"));
        assert_eq!(expectation, WriteExpectation::Revision("a".repeat(64)));
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&data_base64)
                .unwrap(),
            b"data"
        );
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: Some(write_stamp(&"a".repeat(64))),
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::CommitWrite { ticket } = request else {
            panic!("expected a commit request");
        };
        assert_eq!(ticket, DecimalU64::new(1));
        write_record(
            &mut peer,
            &ClientReply::Written {
                ticket,
                metadata: write_stamp(&"b".repeat(64)),
            },
        )
        .unwrap();
    });
    let authorized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = authorized.clone();
    let metadata = connection
        .write_file(
            Path::new("file"),
            b"data",
            Some(&"a".repeat(64)),
            |original| {
                // The gate sees the verified prepared original, not a
                // re-stat: the exact revision that was requested.
                assert_eq!(original.unwrap().revision, "a".repeat(64).as_str());
                observed.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            },
        )
        .unwrap();
    assert!(authorized.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(metadata.revision, "b".repeat(64));
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_prepared_reply_that_ignores_the_expectation_retires_the_stream() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        // The peer reports an original that does not satisfy the
        // expected revision it was sent.
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: Some(write_stamp(&"b".repeat(64))),
            },
        )
        .unwrap();
    });
    let error = connection
        .write_file(Path::new("file"), b"data", Some(&"a".repeat(64)), |_| {
            panic!("the gate never runs on an untruthful prepare")
        })
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_denied_commit_gate_aborts_the_stage_and_never_commits() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: None,
            },
        )
        .unwrap();
        // The only frame after a gate denial is the abort — never a
        // commit.
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::AbortWrite { ticket } = request else {
            panic!("expected an abort request, never a commit");
        };
        assert_eq!(ticket, DecimalU64::new(1));
        write_record(&mut peer, &ClientReply::WriteAborted { ticket }).unwrap();
    });
    let error = connection
        .write_file(Path::new("file"), b"data", None, |original| {
            assert!(original.is_none());
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "gate"))
        })
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_lost_commit_acknowledgement_reports_an_unknown_outcome() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: Some(write_stamp(&"a".repeat(64))),
            },
        )
        .unwrap();
        // The commit request is consumed and the peer vanishes before
        // its reply: exactly one commit frame, then silence — the
        // publish may or may not have happened and no retry can tell.
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::CommitWrite { ticket } = request else {
            panic!("expected a commit request");
        };
        assert_eq!(ticket, DecimalU64::new(1));
        peer.shutdown(std::net::Shutdown::Both).unwrap();
    });
    let error = connection
        .write_file(
            Path::new("file"),
            b"data",
            Some(&"a".repeat(64)),
            |_| Ok(()),
        )
        .unwrap_err();
    assert_outcome_unknown(&error);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn an_explicit_commit_unknown_reports_an_unknown_outcome() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: None,
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::CommitWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::CommitUnknown {
                ticket: DecimalU64::new(1),
            },
        )
        .unwrap();
    });
    let error = connection
        .write_file(Path::new("file"), b"data", None, |_| Ok(()))
        .unwrap_err();
    assert_outcome_unknown(&error);
    // A correctly-ticketed `CommitUnknown` is valid syntax but never
    // a safe ordinary outcome — the publish may have crossed and the
    // ticket is consumed, so the connection retires like every other
    // post-commit failure.
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_written_receipt_with_the_wrong_size_reports_an_unknown_outcome() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: None,
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::CommitWrite { .. }));
        // The ticket is right and the stamp is well-formed, but the
        // size describes different bytes than were submitted: the
        // publish cannot be proven, so the outcome is unknown.
        write_record(
            &mut peer,
            &ClientReply::Written {
                ticket: DecimalU64::new(1),
                metadata: FileStamp {
                    size: DecimalU64::new(3),
                    revision: "b".repeat(64),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
    });
    let error = connection
        .write_file(Path::new("file"), b"data", None, |_| Ok(()))
        .unwrap_err();
    assert_outcome_unknown(&error);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_mismatched_commit_ticket_reports_an_unknown_outcome() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: None,
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::CommitWrite { .. }));
        // A `Written` under a foreign ticket after the commit byte is
        // unknowable — the reply accounting can never be trusted.
        write_record(
            &mut peer,
            &ClientReply::Written {
                ticket: DecimalU64::new(9),
                metadata: write_stamp(&"b".repeat(64)),
            },
        )
        .unwrap();
    });
    let error = connection
        .write_file(Path::new("file"), b"data", None, |_| Ok(()))
        .unwrap_err();
    assert_outcome_unknown(&error);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_prepared_mutation_never_crosses_to_another_connection() {
    let (binding_a, hello_a) = write_rig();
    let (mut connection_a, peer_a) = live(&binding_a, &hello_a);
    let (binding_b, hello_b) = write_rig();
    let (mut connection_b, peer_b) = live(&binding_b, &hello_b);

    let script = |mut peer: UnixStream| {
        thread::spawn(move || {
            let request: ClientRequest = read_record(&mut peer).unwrap();
            assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
            write_record(
                &mut peer,
                &ClientReply::WritePrepared {
                    ticket: DecimalU64::new(1),
                    original: None,
                },
            )
            .unwrap();
            // The foreign-ticket refusals place no frame at all: the
            // one and only request after the prepare is the
            // connection's own commit — a refused call that still
            // touched the wire would arrive here first and fail the
            // shape check, or arrive later and fail the tail check.
            let request: ClientRequest = read_record(&mut peer).unwrap();
            let ClientRequest::CommitWrite { ticket } = request else {
                panic!("expected a commit request");
            };
            assert_eq!(ticket, DecimalU64::new(1));
            write_record(
                &mut peer,
                &ClientReply::Written {
                    ticket,
                    metadata: write_stamp(&"b".repeat(64)),
                },
            )
            .unwrap();
            peer.set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            assert!(read_record::<ClientRequest>(&mut peer).is_err());
        })
    };
    let answer_a = script(peer_a);
    let answer_b = script(peer_b);

    let admit = || Ok(());
    let sent = || ();
    let prepared_a = connection_a
        .prepare_write(
            Path::new("file"),
            &WriteExpectation::Current,
            b"data",
            None,
            &admit,
            &sent,
        )
        .unwrap();
    let prepared_b = connection_b
        .prepare_write(
            Path::new("file"),
            &WriteExpectation::Current,
            b"data",
            None,
            &admit,
            &sent,
        )
        .unwrap();
    // Both peers issued ticket 1 — on different connections.
    assert_eq!(prepared_a.ticket, prepared_b.ticket);

    let error = connection_a
        .commit_prepared(&prepared_b, None, &admit, &sent)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    let error = connection_b
        .abort_prepared(&prepared_a, None, &admit, &sent)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

    // Neither preparation was spent by the refused cross-connection
    // calls: each connection still commits its own ticket.
    connection_a
        .commit_prepared(&prepared_a, None, &admit, &sent)
        .unwrap();
    connection_b
        .commit_prepared(&prepared_b, None, &admit, &sent)
        .unwrap();
    answer_a.join().unwrap();
    answer_b.join().unwrap();
    assert_eq!(binding_a.status().phase, BindingPhase::Live);
    assert_eq!(binding_b.status().phase, BindingPhase::Live);
}

#[test]
fn a_revoked_binding_after_prepare_sends_no_commit_frame() {
    let (binding, hello) = write_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::WritePrepared {
                ticket: DecimalU64::new(1),
                original: None,
            },
        )
        .unwrap();
        peer
    });
    let error = connection
        .write_file(Path::new("file"), b"data", None, |_| {
            // The caller's last gate revokes the binding between the
            // prepared reply and the commit dispatch: the commit
            // frame is gated out before its first byte.
            binding.revoke();
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    let mut peer = answer.join().unwrap();
    // Zero commit frames: the revoke shutdown already closed the
    // physical stream, so the peer observes EOF — never a request.
    let mut buffer = [0u8; 16];
    assert_eq!(peer.read(&mut buffer).unwrap(), 0);
    assert_eq!(binding.status().phase, BindingPhase::Revoked);
}

fn lines_rig() -> (Binding, Hello) {
    reserved(BTreeSet::from([
        Capability::Connection,
        Capability::ReadLines,
    ]))
}

fn lines_stamp(size: u64, revision: &str) -> FileStamp {
    FileStamp {
        size: DecimalU64::new(size),
        revision: revision.to_owned(),
        identity: String::from("1:1"),
        modified_at_ms: None,
    }
}

/// A `Lines` reply shape the validator must accept: six source bytes
/// as `"a\nb\nc"`, three addressable lines, one shown element.
fn lines_reply(shown: &str, shown_lines: u32, total: u64, end: LineEnd) -> ClientReply {
    ClientReply::Lines {
        metadata: lines_stamp(6, &"a".repeat(64)),
        text: shown.to_owned(),
        total_lines: DecimalU64::new(total),
        shown_lines,
        end,
    }
}

#[test]
fn a_peer_without_the_lines_capability_gets_no_request() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
    let (mut connection, mut peer) = live(&binding, &hello);
    let error = connection
        .read_lines(Path::new("file"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    peer.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert!(read_record::<ClientRequest>(&mut peer).is_err());
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_zero_lines_request_never_reaches_the_wire() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    for (offset, limit) in [(0, None), (1, Some(0))] {
        let error = connection
            .read_lines(Path::new("file"), offset, limit, None)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    peer.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert!(read_record::<ClientRequest>(&mut peer).is_err());
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_lines_page_is_verified_against_the_request() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let expected = "a".repeat(64);
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadLines {
            relative_path,
            offset,
            limit,
            expected_revision,
        } = request
        else {
            panic!("expected a lines request");
        };
        assert_eq!(relative_path, PathBuf::from("file"));
        assert_eq!(offset.get(), 2);
        assert_eq!(limit, Some(DecimalU64::new(1)));
        assert_eq!(expected_revision.as_deref(), Some(expected.as_str()));
        write_record(&mut peer, &lines_reply("b", 1, 3, LineEnd::RequestedLimit)).unwrap();
    });
    let page = connection
        .read_lines(Path::new("file"), 2, Some(1), Some(&"a".repeat(64)))
        .unwrap();
    answer.join().unwrap();
    assert_eq!(page.text, "b");
    assert_eq!(page.total_lines, 3);
    assert_eq!(page.shown_lines, 1);
    assert_eq!(page.end, LineEnd::RequestedLimit);
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_complete_lines_page_may_show_the_trailing_empty_line() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // A source ending in a newline keeps its trailing empty
        // element on a complete page: `"a\n"` shows 2 elements.
        write_record(
            &mut peer,
            &ClientReply::Lines {
                metadata: lines_stamp(2, &"a".repeat(64)),
                text: String::from("a\n"),
                total_lines: DecimalU64::new(2),
                shown_lines: 2,
                end: LineEnd::Complete,
            },
        )
        .unwrap();
    });
    let page = connection
        .read_lines(Path::new("file"), 1, None, None)
        .unwrap();
    answer.join().unwrap();
    assert_eq!(page.text, "a\n");
    assert_eq!(page.shown_lines, 2);
    assert_eq!(page.end, LineEnd::Complete);
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn an_untruthful_lines_end_retires_the_stream() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // "Complete" while lines remain: the end reason cannot
        // account for the page.
        write_record(&mut peer, &lines_reply("a", 1, 3, LineEnd::Complete)).unwrap();
    });
    let error = connection
        .read_lines(Path::new("file"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_lines_reply_whose_text_contradicts_shown_retires_the_stream() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // `shown` claims two elements; the text holds one.
        write_record(&mut peer, &lines_reply("a", 2, 3, LineEnd::LineLimit)).unwrap();
    });
    let error = connection
        .read_lines(Path::new("file"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_lines_reply_with_zero_total_retires_the_stream() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // Even an empty source addresses one line: zero is impossible.
        write_record(&mut peer, &lines_reply("", 0, 0, LineEnd::OffsetPastEnd)).unwrap();
    });
    let error = connection
        .read_lines(Path::new("file"), 5, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_lines_reply_with_a_stale_revision_retires_the_stream() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        write_record(
            &mut peer,
            &ClientReply::Lines {
                metadata: lines_stamp(6, &"b".repeat(64)),
                text: String::from("a"),
                total_lines: DecimalU64::new(1),
                shown_lines: 1,
                end: LineEnd::Complete,
            },
        )
        .unwrap();
    });
    let error = connection
        .read_lines(Path::new("file"), 1, None, Some(&"a".repeat(64)))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_lines_reply_at_the_offset_space_edge_does_not_panic() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadLines { offset, .. } = request else {
            panic!("expected a lines request");
        };
        assert_eq!(offset.get(), u64::MAX);
        // `u64::MAX` is a legitimate out-of-range offset: the honest
        // `OffsetPastEnd` reply must survive validation — arithmetic
        // that overflowed here would panic, not retire.
        write_record(&mut peer, &lines_reply("", 0, 2, LineEnd::OffsetPastEnd)).unwrap();
    });
    let page = connection
        .read_lines(Path::new("file"), u64::MAX, None, None)
        .unwrap();
    answer.join().unwrap();
    assert_eq!(page.end, LineEnd::OffsetPastEnd);
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_complete_lines_reply_that_violates_the_limit_retires() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // `Complete` claims the window reached the source end, but
        // the request asked for one line and two arrived — the
        // accounting cannot be trusted.
        write_record(&mut peer, &lines_reply("a\nb", 2, 3, LineEnd::Complete)).unwrap();
    });
    let error = connection
        .read_lines(Path::new("file"), 1, Some(1), None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_budget_stop_that_consumed_the_window_retires() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // `ByteLimit` claims a budget stop, yet the shown count
        // consumed the whole requested window — nothing was left
        // for a budget to stop on.
        write_record(
            &mut peer,
            &lines_reply("a\nb\nc\nd\ne", 5, 10, LineEnd::ByteLimit),
        )
        .unwrap();
    });
    let error = connection
        .read_lines(Path::new("file"), 1, Some(5), None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_2001_element_page_without_the_trailing_newline_retires() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // `shown_lines` may exceed `MAX_TEXT_LINES` only for the
        // trailing-empty page: 2001 nonempty elements is a lie.
        let text = "x\n".repeat(MAX_TEXT_LINES) + "x";
        write_record(
            &mut peer,
            &ClientReply::Lines {
                metadata: lines_stamp(5_000_000, &"a".repeat(64)),
                text,
                total_lines: DecimalU64::new(2_001),
                shown_lines: u32::try_from(MAX_TEXT_LINES + 1).unwrap(),
                end: LineEnd::Complete,
            },
        )
        .unwrap();
    });
    let error = connection
        .read_lines(Path::new("file"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_requested_limit_may_keep_the_trailing_empty_element() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // The file `("x\n" * 2000) + "\nrest"`: the caller's limit
        // selects the empty element after the 2000th line — exempt
        // from the budgets exactly like the source's own trailing
        // empty, so 2001 shown elements ending in a newline is a
        // legitimate `RequestedLimit` page.
        write_record(
            &mut peer,
            &ClientReply::Lines {
                metadata: lines_stamp(4_005, &"a".repeat(64)),
                text: "x\n".repeat(MAX_TEXT_LINES),
                total_lines: DecimalU64::new(u64::try_from(MAX_TEXT_LINES + 2).unwrap()),
                shown_lines: u32::try_from(MAX_TEXT_LINES + 1).unwrap(),
                end: LineEnd::RequestedLimit,
            },
        )
        .unwrap();
    });
    let page = connection
        .read_lines(
            Path::new("file"),
            1,
            Some(u64::try_from(MAX_TEXT_LINES + 1).unwrap()),
            None,
        )
        .unwrap();
    answer.join().unwrap();
    assert_eq!(page.shown_lines, u32::try_from(MAX_TEXT_LINES + 1).unwrap());
    assert!(page.text.ends_with('\n'));
    assert_eq!(page.end, LineEnd::RequestedLimit);
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn an_invalid_utf8_error_keeps_the_connection_live() {
    let (binding, hello) = lines_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        write_record(
            &mut peer,
            &ClientReply::Error {
                kind: ReadError::InvalidUtf8,
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    // The semantic refusal maps to InvalidData but never retires:
    // the same connection still answers Ping afterward.
    let error = connection
        .read_lines(Path::new("file"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

fn document_rig() -> (Binding, Hello) {
    reserved(BTreeSet::from([
        Capability::Connection,
        Capability::DocumentPages,
    ]))
}

/// A `DocumentPage` reply shape the validator must accept: a 512-byte
/// compressed source decoding to a six-byte `"a\nb\nc"` Markdown
/// whose logical length — not the source size — carries the page
/// accounting.
fn document_reply(
    logical_bytes: u64,
    text: &str,
    shown_lines: u32,
    total: u64,
    end: LineEnd,
) -> ClientReply {
    ClientReply::DocumentPage {
        source: lines_stamp(512, &"a".repeat(64)),
        logical_bytes: DecimalU64::new(logical_bytes),
        text: text.to_owned(),
        total_lines: DecimalU64::new(total),
        shown_lines,
        end,
    }
}

#[test]
fn a_peer_without_the_document_capability_gets_no_request() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
    let (mut connection, mut peer) = live(&binding, &hello);
    let error = connection
        .read_document(Path::new("report.pdf"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    peer.set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert!(read_record::<ClientRequest>(&mut peer).is_err());
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_document_page_is_verified_against_the_request() {
    let (binding, hello) = document_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let expected = "a".repeat(64);
    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::ReadDocument {
            relative_path,
            offset,
            limit,
            expected_revision,
        } = request
        else {
            panic!("expected a document request");
        };
        assert_eq!(relative_path, PathBuf::from("report.pdf"));
        assert_eq!(offset.get(), 2);
        assert_eq!(limit, Some(DecimalU64::new(1)));
        assert_eq!(expected_revision.as_deref(), Some(expected.as_str()));
        // The decoded text is longer than the page and independent
        // of the compressed source's stamped size.
        write_record(
            &mut peer,
            &document_reply(900, "b", 1, 3, LineEnd::RequestedLimit),
        )
        .unwrap();
    });
    let page = connection
        .read_document(Path::new("report.pdf"), 2, Some(1), Some(&"a".repeat(64)))
        .unwrap();
    answer.join().unwrap();
    assert_eq!(page.text, "b");
    assert_eq!(page.total_lines, 3);
    assert_eq!(page.shown_lines, 1);
    assert_eq!(page.end, LineEnd::RequestedLimit);
    assert_eq!(page.logical_bytes, 900);
    assert_eq!(page.source.size.get(), 512);
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_wrong_document_reply_kind_retires_the_stream() {
    let (binding, hello) = document_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // A `Lines` reply to a `ReadDocument` request is a protocol
        // violation: the reply accounting cannot be trusted.
        write_record(&mut peer, &lines_reply("a", 1, 1, LineEnd::Complete)).unwrap();
    });
    let error = connection
        .read_document(Path::new("report.pdf"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_document_reply_with_a_conflicting_source_retires_the_stream() {
    let (binding, hello) = document_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        write_record(
            &mut peer,
            &ClientReply::DocumentPage {
                source: lines_stamp(512, &"b".repeat(64)),
                logical_bytes: DecimalU64::new(1),
                text: String::from("a"),
                total_lines: DecimalU64::new(1),
                shown_lines: 1,
                end: LineEnd::Complete,
            },
        )
        .unwrap();
    });
    let error = connection
        .read_document(Path::new("report.pdf"), 1, None, Some(&"a".repeat(64)))
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_document_reply_past_the_bounds_retires_the_stream() {
    let (binding, hello) = document_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        // A source the Client could never have decoded and a
        // projection beyond the decoder's own cap are both
        // untruthful — one exchange each, then the peer drains.
        let _: ClientRequest = read_record(&mut peer).unwrap();
        write_record(
            &mut peer,
            &ClientReply::DocumentPage {
                source: lines_stamp(
                    u64::try_from(MAX_DOCUMENT_SOURCE_BYTES + 1).unwrap(),
                    &"a".repeat(64),
                ),
                logical_bytes: DecimalU64::new(1),
                text: String::from("a"),
                total_lines: DecimalU64::new(1),
                shown_lines: 1,
                end: LineEnd::Complete,
            },
        )
        .unwrap();
    });
    let error = connection
        .read_document(Path::new("report.pdf"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_document_reply_past_the_logical_bound_retires_the_stream() {
    let (binding, hello) = document_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        write_record(
            &mut peer,
            &document_reply(MAX_DOCUMENT_LOGICAL_BYTES + 1, "a", 1, 1, LineEnd::Complete),
        )
        .unwrap();
    });
    let error = connection
        .read_document(Path::new("report.pdf"), 1, None, None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_document_reply_shares_the_lines_accounting() {
    let (binding, hello) = document_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        let _: ClientRequest = read_record(&mut peer).unwrap();
        // `Complete` claims the window reached the end of the
        // decoded text, but the request asked for one line and two
        // arrived — the shared accounting catches it.
        write_record(
            &mut peer,
            &document_reply(900, "a\nb", 2, 3, LineEnd::Complete),
        )
        .unwrap();
    });
    let error = connection
        .read_document(Path::new("report.pdf"), 1, Some(1), None)
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}

#[test]
fn a_document_semantic_error_keeps_the_connection_live() {
    let (binding, hello) = document_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let answer = thread::spawn(move || {
        for kind in [
            ReadError::NoTextLayer,
            ReadError::DocumentDecodeFailed,
            ReadError::UnsupportedDocument,
            ReadError::DocumentTooLarge,
        ] {
            let _: ClientRequest = read_record(&mut peer).unwrap();
            write_record(&mut peer, &ClientReply::Error { kind }).unwrap();
        }
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    // Every document refusal is a distinct stable kind — never
    // conflated — and none retires a healthy connection.
    assert_eq!(
        connection
            .read_document(Path::new("scanned.pdf"), 1, None, None)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        connection
            .read_document(Path::new("broken.docx"), 1, None, None)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        connection
            .read_document(Path::new("notes.md"), 1, None, None)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Unsupported
    );
    assert_eq!(
        connection
            .read_document(Path::new("huge.xlsx"), 1, None, None)
            .unwrap_err()
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_conditional_edit_prepares_authorizes_and_commits() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let revision = "a".repeat(64);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::PrepareEdit {
            relative_path,
            expectation,
            replacements,
        } = request
        else {
            panic!("expected a prepare-edit request");
        };
        assert_eq!(relative_path, Path::new("file"));
        assert_eq!(expectation, WriteExpectation::Revision("a".repeat(64)));
        assert_eq!(replacements.len(), 1);
        write_record(
            &mut peer,
            &ClientReply::EditPrepared {
                ticket: DecimalU64::new(1),
                original: write_stamp(&"a".repeat(64)),
                published_size: DecimalU64::new(5),
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::CommitWrite { ticket } = request else {
            panic!("expected a commit request");
        };
        assert_eq!(ticket, DecimalU64::new(1));
        write_record(
            &mut peer,
            &ClientReply::Written {
                ticket,
                metadata: FileStamp {
                    size: DecimalU64::new(5),
                    revision: "b".repeat(64),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
            },
        )
        .unwrap();
    });
    let published = connection
        .edit_file(
            Path::new("file"),
            &revision,
            &[replacement("old", "newer")],
            |original| {
                assert_eq!(original.revision, "a".repeat(64));
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(published.size.get(), 5);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn an_edit_refusal_projects_the_typed_reason_and_stays_live() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
        write_record(
            &mut peer,
            &ClientReply::EditRefused {
                reason: EditRefusal::Ambiguous {
                    index: 0,
                    occurrences: 3,
                },
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert_eq!(request, ClientRequest::Ping);
        write_record(&mut peer, &ClientReply::Pong).unwrap();
    });
    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[replacement("x", "y")],
            |_| panic!("a semantic refusal never reaches the gate"),
        )
        .unwrap_err();
    let refused = error.get_ref().unwrap().downcast_ref::<RefusedEdit>();
    assert_eq!(
        refused.map(|refused| refused.0),
        Some(EditRefusal::Ambiguous {
            index: 0,
            occurrences: 3
        })
    );
    connection.ping().unwrap();
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_contradicting_edit_refusal_retires_the_connection() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
        // Index 4 names a replacement the request never carried.
        write_record(
            &mut peer,
            &ClientReply::EditRefused {
                reason: EditRefusal::Missing { index: 4 },
            },
        )
        .unwrap();
    });
    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[replacement("x", "y")],
            |_| panic!("a malformed refusal never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
}

#[test]
fn an_edit_prepared_reply_is_validated_before_the_gate() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
        // A published size above the file cap can never be committed.
        write_record(
            &mut peer,
            &ClientReply::EditPrepared {
                ticket: DecimalU64::new(1),
                original: write_stamp(&"a".repeat(64)),
                published_size: DecimalU64::new(MAX_EDIT_FILE_BYTES as u64 + 1),
            },
        )
        .unwrap();
    });
    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[replacement("x", "y")],
            |_| panic!("an oversized prepare never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
}

#[test]
fn a_denied_edit_gate_aborts_under_the_same_deadline() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
        write_record(
            &mut peer,
            &ClientReply::EditPrepared {
                ticket: DecimalU64::new(1),
                original: write_stamp(&"a".repeat(64)),
                published_size: DecimalU64::new(5),
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        let ClientRequest::AbortWrite { ticket } = request else {
            panic!("expected an abort request, never a commit");
        };
        write_record(&mut peer, &ClientReply::WriteAborted { ticket }).unwrap();
    });
    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[replacement("x", "y")],
            |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "gate")),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn an_edit_commit_unknown_retires_the_connection() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
        write_record(
            &mut peer,
            &ClientReply::EditPrepared {
                ticket: DecimalU64::new(1),
                original: write_stamp(&"a".repeat(64)),
                published_size: DecimalU64::new(5),
            },
        )
        .unwrap();
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::CommitWrite { .. }));
        write_record(
            &mut peer,
            &ClientReply::CommitUnknown {
                ticket: DecimalU64::new(1),
            },
        )
        .unwrap();
    });
    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[replacement("x", "y")],
            |_| Ok(()),
        )
        .unwrap_err();
    assert_outcome_unknown(&error);
    answer.join().unwrap();
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err());
}
#[test]
fn an_over_cap_edit_original_retires_before_the_gate() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let authorized = std::sync::atomic::AtomicUsize::new(0);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
        // A valid-looking stamp claiming a source the Client could
        // never have decoded.
        write_record(
            &mut peer,
            &ClientReply::EditPrepared {
                ticket: DecimalU64::new(1),
                original: FileStamp {
                    size: DecimalU64::new(MAX_EDIT_FILE_BYTES as u64 + 1),
                    revision: "a".repeat(64),
                    identity: String::from("1:1"),
                    modified_at_ms: None,
                },
                published_size: DecimalU64::new(4),
            },
        )
        .unwrap();
    });
    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[replacement("x", "y")],
            |_| {
                authorized.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(authorized.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(binding.status().phase, BindingPhase::Detached);
}

#[test]
fn an_empty_anchor_refusal_on_a_nonempty_anchor_retires() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);
    let authorized = std::sync::atomic::AtomicUsize::new(0);

    let answer = thread::spawn(move || {
        let request: ClientRequest = read_record(&mut peer).unwrap();
        assert!(matches!(request, ClientRequest::PrepareEdit { .. }));
        // The request's anchor is nonempty — the refusal contradicts
        // the request it claims to answer.
        write_record(
            &mut peer,
            &ClientReply::EditRefused {
                reason: EditRefusal::EmptyAnchor { index: 0 },
            },
        )
        .unwrap();
    });
    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[replacement("x", "y")],
            |_| {
                authorized.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    answer.join().unwrap();
    assert_eq!(authorized.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(binding.status().phase, BindingPhase::Detached);
}

#[test]
fn an_empty_anchor_in_the_request_never_reaches_the_wire() {
    let (binding, hello) = edit_rig();
    let (mut connection, mut peer) = live(&binding, &hello);

    let error = connection
        .edit_file(
            Path::new("file"),
            &"a".repeat(64),
            &[
                replacement("x", "y"),
                TextReplacement {
                    old_text: String::new(),
                    new_text: "z".to_owned(),
                },
            ],
            |_| panic!("a malformed request never reaches the gate"),
        )
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    // Quiet wire: the peer sees nothing.
    peer.set_nonblocking(true).unwrap();
    let mut buffer = [0u8; 16];
    assert_eq!(
        peer.read(&mut buffer).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(binding.status().phase, BindingPhase::Live);
}
/// A control peer whose receipts the test gates by hand: each
/// delivered request parks on `gate` before its correct receipt
/// rides the next poll — the deterministic way to hold commands
/// in-flight and queued.
fn control_peer_loop_gated(
    channel: &mut rustx_execution_transport::RecordChannel,
    gate: &mpsc::Receiver<()>,
) {
    let mut sequence = 1_u64;
    let mut receipt: Option<ControlReply> = None;
    loop {
        let deadline = Instant::now() + Duration::from_secs(30);
        let poll = ControlPoll {
            sequence: DecimalU64::new(sequence),
            receipt: receipt.take(),
            health: DecimalU64::new(sequence),
        };
        if channel
            .send(&encode_record(&poll).unwrap(), Some(deadline))
            .is_err()
        {
            return;
        }
        let Ok(turn) = channel.receive::<ControlTurn>(Some(deadline)) else {
            return;
        };
        if turn.sequence != poll.sequence || turn.health != poll.health {
            return;
        }
        sequence += 1;
        let Some(request) = turn.request else {
            continue;
        };
        let done = matches!(request, ControlRequest::Revoke);
        // The receipt waits for the test's explicit release — the
        // command stays genuinely in-flight until then.
        if gate.recv_timeout(Duration::from_secs(30)).is_err() {
            return;
        }
        receipt = Some(match request {
            ControlRequest::Health { nonce } => ControlReply::Healthy { nonce },
            ControlRequest::CancelMutation { ticket } => ControlReply::MutationCancelled {
                ticket,
                disposition: CancelDisposition::Unknown,
            },
            ControlRequest::Revoke => ControlReply::Revoked,
            ControlRequest::StopWatch { id } => ControlReply::WatchStopped { id, settled: true },
            ControlRequest::CancelProcess { key } => ControlReply::ProcessCancelled {
                key,
                disposition: rustx_execution_protocol::ProcessCancelDisposition::Unknown,
            },
        });
        if done {
            let poll = ControlPoll {
                sequence: DecimalU64::new(sequence),
                receipt,
                health: DecimalU64::new(sequence),
            };
            let _ = channel.send(
                &encode_record(&poll).unwrap(),
                Some(Instant::now() + Duration::from_secs(5)),
            );
            return;
        }
    }
}

/// Pairs a control lane whose peer runs the gated loop — the test
/// releases each receipt through the returned sender.
fn pair_control_half_gated(
    binding: &Binding,
    hello: &Hello,
    challenge: &PairingNonce,
) -> (LocalConnection, mpsc::Sender<()>) {
    let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
    write_record(&mut control_peer, &control_hello(hello, challenge.clone())).unwrap();
    let accepted = binding.accept_local(control_stream).unwrap();
    let ChannelAccept::Live(connection) = accepted else {
        panic!("the paired control lane commits the connection");
    };
    let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
    assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
    let (gate, releases) = mpsc::channel();
    thread::spawn(move || {
        let mut channel = RecordChannel::unix(control_peer);
        control_peer_loop_gated(&mut channel, &releases);
    });
    (connection, gate)
}

#[test]
fn control_lane_progresses_while_a_data_write_is_physically_blocked() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::ConditionalWrite]));
    let (mut peer, stream) = UnixStream::pair().unwrap();
    // A 4 KiB kernel send buffer on the server's lane: the 64 KiB
    // `PrepareWrite` frame cannot drain while the peer never reads —
    // the dispatch's `send` is physically blocked inside the kernel
    // write, not merely parked on a semaphore.
    nix::sys::socket::setsockopt(&stream, nix::sys::socket::sockopt::SndBuf, &4096).unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    let mut connection = live_data_half(&binding, &hello, stream, &mut peer);

    let payload = vec![b'x'; MAX_RAW_CHUNK_BYTES];
    let encoded_len = encode_record(&ClientRequest::PrepareWrite {
        relative_path: Path::new("file").to_path_buf(),
        expectation: WriteExpectation::Absent,
        data_base64: base64::engine::general_purpose::STANDARD.encode(&payload),
    })
    .unwrap()
    .as_bytes()
    .len();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dispatched = done.clone();
    let exchange = thread::spawn(move || {
        let result = connection.prepare_write(
            Path::new("file"),
            &WriteExpectation::Absent,
            &payload,
            None,
            &|| Ok(()),
            &|| (),
        );
        dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
        result
    });
    // Give the kernel write room to stall, then prove it has: a
    // strict prefix of the encoded frame sits peeked-but-unread in
    // the peer's kernel queue — MSG_PEEK never consumes, so the
    // blocked writer cannot have pushed the tail either — and the
    // exchange has not returned.
    std::thread::sleep(Duration::from_millis(300));
    let mut peeked = vec![0u8; encoded_len];
    let queued = nix::sys::socket::recv(
        std::os::unix::io::AsRawFd::as_raw_fd(&peer),
        &mut peeked,
        nix::sys::socket::MsgFlags::MSG_PEEK | nix::sys::socket::MsgFlags::MSG_DONTWAIT,
    )
    .unwrap();
    assert!(
        queued > 0 && queued < encoded_len,
        "a strict prefix of the {encoded_len}-byte frame is queued: {queued} bytes"
    );
    assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

    // The independent lane advances anyway: fresh health proofs and
    // a real cancel receipt — while the data write is still stuck.
    let control = binding.control().expect("a live pair has a control handle");
    control.health().unwrap();
    assert_eq!(
        control.cancel_mutation(DecimalU64::new(99)).unwrap(),
        CancelDisposition::Unknown
    );
    assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

    // The fence still interrupts the blocked writer without ever
    // waiting on the data lock — the exchange resolves with an
    // error, never a completed write.
    binding.fence().shutdown();
    let outcome = exchange.join().unwrap();
    assert!(
        outcome.is_err(),
        "the fenced exchange must not complete: {outcome:?}"
    );
    assert!(done.load(std::sync::atomic::Ordering::SeqCst));
}

/// One real loopback WebSocket pair admitted through
/// `accept_channel_deadline`, with the buffers squeezed on both
/// ends: the server's send buffer and the client's receive buffer
/// are each capped at 4 KiB — the duplicated handle configures the
/// shared socket — so a 64 KiB frame physically stalls mid-write
/// while the client never reads. Returns the live connection, the
/// held data peer, and a read-only duplicate for kernel peeks.
fn congested_websocket_pair(
    binding: &Binding,
    hello: &Hello,
) -> (
    LocalConnection,
    rustx_execution_transport::RecordChannel,
    ChannelShutdown,
) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let endpoint = || {
        rustx_execution_transport::ExecutionUrl::validate(&format!("ws://{address}/execution/v1"))
            .unwrap()
    };
    let (challenge, client, probe) = thread::scope(|scope| {
        let server = scope.spawn(|| {
            let (stream, _) = listener.accept().unwrap();
            nix::sys::socket::setsockopt(&stream, nix::sys::socket::sockopt::SndBuf, &4096)
                .unwrap();
            let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
            binding.accept_channel_deadline(channel, deadline).unwrap()
        });
        let mut client = rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
        let probe = client.shutdown_handle().unwrap();
        nix::sys::socket::setsockopt(
            &probe.as_fd().unwrap(),
            nix::sys::socket::sockopt::RcvBuf,
            &4096,
        )
        .unwrap();
        client
            .send(
                &encode_record(&PeerHello::Data(hello.clone())).unwrap(),
                Some(deadline),
            )
            .unwrap();
        let reply: DataHelloReply = client.receive(Some(deadline)).unwrap();
        let Ok(data) = reply else {
            panic!("the honest data Hello is admitted");
        };
        assert!(matches!(server.join().unwrap(), ChannelAccept::Pending));
        (data.challenge, client, probe)
    });
    let connection = thread::scope(|scope| {
        let server = scope.spawn(|| {
            let (stream, _) = listener.accept().unwrap();
            let channel = rustx_execution_transport::accept(stream, deadline).unwrap();
            binding.accept_channel_deadline(channel, deadline).unwrap()
        });
        let mut control = rustx_execution_transport::connect(&endpoint(), deadline, None).unwrap();
        control
            .send(
                &encode_record(&control_hello(hello, challenge.clone())).unwrap(),
                Some(deadline),
            )
            .unwrap();
        let reply: ControlHelloReply = control.receive(Some(deadline)).unwrap();
        assert!(reply.is_ok(), "the honest control lane commits: {reply:?}");
        let ChannelAccept::Live(connection) = server.join().unwrap() else {
            panic!("the paired control lane commits the connection");
        };
        thread::spawn(move || control_peer_loop(&mut control));
        connection
    });
    (connection, client, probe)
}

#[test]
fn control_lane_progresses_while_a_websocket_data_write_is_physically_blocked() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::ConditionalWrite]));
    // The WebSocket client never reads: the 64 KiB `PrepareWrite`
    // frame physically stalls the server's writer.
    let (mut connection, _client, client_probe) = congested_websocket_pair(&binding, &hello);
    let payload = vec![b'x'; MAX_RAW_CHUNK_BYTES];
    let encoded_len = encode_record(&ClientRequest::PrepareWrite {
        relative_path: Path::new("file").to_path_buf(),
        expectation: WriteExpectation::Absent,
        data_base64: base64::engine::general_purpose::STANDARD.encode(&payload),
    })
    .unwrap()
    .as_bytes()
    .len();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dispatched = done.clone();
    let exchange = thread::spawn(move || {
        let result = connection.prepare_write(
            Path::new("file"),
            &WriteExpectation::Absent,
            &payload,
            None,
            &|| Ok(()),
            &|| (),
        );
        dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
        result
    });
    std::thread::sleep(Duration::from_millis(300));
    // Proof the write is inside the kernel, not merely unparked:
    // peek — never consume — the client's receive queue: a strict
    // prefix of the frame sits there while the exchange has not
    // returned, so the tail cannot have been written either.
    let mut peeked = vec![0u8; encoded_len];
    let queued = nix::sys::socket::recv(
        std::os::fd::AsRawFd::as_raw_fd(&client_probe.as_fd().unwrap()),
        &mut peeked,
        nix::sys::socket::MsgFlags::MSG_PEEK | nix::sys::socket::MsgFlags::MSG_DONTWAIT,
    )
    .unwrap();
    assert!(
        queued > 0 && queued < encoded_len,
        "a strict prefix of the {encoded_len}-byte frame is queued: {queued} bytes"
    );
    assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

    let control_handle = binding.control().expect("a live pair has a control handle");
    control_handle.health().unwrap();
    assert_eq!(
        control_handle.cancel_mutation(DecimalU64::new(99)).unwrap(),
        CancelDisposition::Unknown
    );
    assert!(!done.load(std::sync::atomic::Ordering::SeqCst));

    binding.fence().shutdown();
    let outcome = exchange.join().unwrap();
    assert!(
        outcome.is_err(),
        "the fenced exchange must not complete: {outcome:?}"
    );
    assert!(done.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn the_control_command_budget_refuses_the_fifth_command_promptly() {
    let (binding, hello) = reserved(required());
    let (mut peer, stream) = UnixStream::pair().unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(stream).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer).unwrap();
    let Ok(data) = reply else {
        panic!("the honest data Hello is admitted");
    };
    let (connection, gate) = pair_control_half_gated(&binding, &hello, &data.challenge);
    let control = connection
        .control()
        .expect("a live pair has a control handle");

    // Five concurrent health commands over a lane whose peer holds
    // every receipt behind the test's gate: the lane admits at most
    // MAX_CONTROL_COMMANDS queued plus in-flight — exactly one of
    // the five resolves WouldBlock promptly while the four admitted
    // commands stay genuinely pending for their gated receipts.
    let (done_tx, done_rx) = mpsc::channel();
    let mut threads = Vec::new();
    for _ in 0..5 {
        let control = control.clone();
        let done_tx = done_tx.clone();
        threads.push(thread::spawn(move || {
            let result = control.health();
            let _ = done_tx.send(result.map_err(|error| error.kind()));
        }));
    }
    match done_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Err(io::ErrorKind::WouldBlock)) => {}
        other => panic!("the fifth command was not refused promptly: {other:?}"),
    }
    assert!(
        matches!(
            done_rx.recv_timeout(Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "the four admitted commands are still pending, not refused"
    );
    drop(done_rx);
    drop(done_tx);
    // Revocation drains the admitted waiters with the lane's
    // terminal error — every one resolves, never a false success —
    // and the gated peer exits when the lane ends.
    binding.revoke();
    drop(gate);
    for thread in threads {
        thread.join().unwrap();
    }
    drop(connection);
}

#[test]
fn a_mutation_cancel_waits_for_saturated_watch_stop_receipts() {
    let (binding, hello) = reserved(required());
    let (mut data_peer, data_stream) = UnixStream::pair().unwrap();
    write_record(&mut data_peer, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(data_stream).unwrap(),
        ChannelAccept::Pending
    ));
    let data = read_record::<DataHelloReply>(&mut data_peer)
        .unwrap()
        .unwrap();
    let (connection, gate) = pair_control_half_gated(&binding, &hello, &data.challenge);
    let control = connection.control().unwrap();
    let mut watches = Vec::new();
    for serial in 1..=MAX_CONTROL_COMMANDS {
        let id = WatchId::new(NonZeroU64::new(serial as u64).unwrap());
        let receipt = control
            .shared
            .enqueue(ControlRequest::StopWatch { id })
            .unwrap();
        watches.push((id, receipt));
    }
    let (waiting_tx, waiting) = mpsc::channel();
    *control.shared.before_capacity_wait.lock().unwrap() = Some(waiting_tx);
    let ticket = DecimalU64::new(42);
    let cancellation = thread::spawn(move || control.cancel_mutation(ticket));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!cancellation.is_finished());
    for (id, receipt) in watches {
        gate.send(()).unwrap();
        assert_eq!(
            receipt
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap(),
            ControlReply::WatchStopped { id, settled: true }
        );
    }
    gate.send(()).unwrap();
    assert_eq!(
        cancellation.join().unwrap().unwrap(),
        CancelDisposition::Unknown
    );
    assert_eq!(binding.status().phase, BindingPhase::Live);
}

#[test]
fn a_saturated_control_lane_refuses_a_cancel_explicitly_and_delivers_it_once_a_slot_frees() {
    let (binding, hello) = reserved(required());
    let (mut peer, stream) = UnixStream::pair().unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(stream).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer).unwrap();
    let Ok(data) = reply else {
        panic!("the honest data Hello is admitted");
    };
    let (connection, gate) = pair_control_half_gated(&binding, &hello, &data.challenge);
    let control = connection
        .control()
        .expect("a live pair has a control handle");

    // Fill the whole budget synchronously: four admissions are queued
    // before the test continues, so no timing decides the outcome.
    let mut admitted = Vec::new();
    for nonce in 1..=4_u64 {
        admitted.push(
            control
                .shared
                .enqueue(ControlRequest::Health {
                    nonce: DecimalU64::new(nonce),
                })
                .unwrap(),
        );
    }

    // A full budget is an explicit refusal at the deadline — the cancel
    // was never queued, so it cannot be silently lost.
    let key = process_key();
    let refused = control.cancel_process_by(&key, Instant::now()).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);

    // Once the admitted commands resolve, the same cancel is queued,
    // delivered on the lane, and acknowledged.
    for _ in 0..4 {
        gate.send(()).unwrap();
    }
    for receipt in admitted {
        receipt
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
    }
    let delivered = {
        let control = control.clone();
        let key = key.clone();
        thread::spawn(move || {
            control.cancel_process_by(&key, Instant::now() + Duration::from_secs(5))
        })
    };
    gate.send(()).unwrap();
    assert_eq!(
        delivered.join().unwrap().unwrap(),
        rustx_execution_protocol::ProcessCancelDisposition::Unknown
    );
    binding.revoke();
    drop(gate);
    drop(connection);
}

#[test]
fn a_replayed_or_crossed_pairing_nonce_never_pairs() {
    let (binding, hello) = reserved(required());
    let (mut peer, stream) = UnixStream::pair().unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(stream).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer).unwrap();
    let Ok(data) = reply else {
        panic!("the honest data Hello is admitted");
    };
    let connection = pair_control_half(&binding, &hello, &data.challenge);
    assert_eq!(binding.status().phase, BindingPhase::Live);

    // A second control lane replaying the consumed nonce: the
    // attempt is spent at commit and the binding is already Live —
    // the replay meets the typed already-connected reject, never a
    // second pair.
    let (mut replayed_peer, replayed_stream) = UnixStream::pair().unwrap();
    write_record(
        &mut replayed_peer,
        &control_hello(&hello, data.challenge.clone()),
    )
    .unwrap();
    assert!(matches!(
        binding.accept_local(replayed_stream).unwrap(),
        ChannelAccept::Rejected(Reject::AlreadyConnected)
    ));
    let reply: ControlHelloReply = read_record(&mut replayed_peer).unwrap();
    assert!(matches!(reply, Err(Reject::AlreadyConnected)));
    drop(connection);

    // A foreign binding's challenge can never cross-wire: Session B's
    // nonce is meaningless under Session A's attempt.
    let (binding_a, hello_a) = reserved(required());
    let mut reservation_b = reservation();
    reservation_b.session = SessionId::new("session-2");
    let (binding_b, credential_b) = Binding::reserve(reservation_b).unwrap();
    let mut hello_b = valid_hello(&credential_b);
    hello_b.session = SessionId::new("session-2");

    let (mut peer_a, stream_a) = UnixStream::pair().unwrap();
    write_record(&mut peer_a, &PeerHello::Data(hello_a.clone())).unwrap();
    assert!(matches!(
        binding_a.accept_local(stream_a).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer_a).unwrap();
    let Ok(data_a) = reply else {
        panic!("session A's data half is admitted");
    };

    let (mut peer_b, stream_b) = UnixStream::pair().unwrap();
    write_record(&mut peer_b, &PeerHello::Data(hello_b.clone())).unwrap();
    assert!(matches!(
        binding_b.accept_local(stream_b).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer_b).unwrap();
    let Ok(data_b) = reply else {
        panic!("session B's data half is admitted");
    };

    // B's nonce against A's attempt and A's nonce against B's: both
    // are typed pairing rejects — the reservations stay Reserved.
    let (mut cross_peer, cross_stream) = UnixStream::pair().unwrap();
    write_record(
        &mut cross_peer,
        &control_hello(&hello_a, data_b.challenge.clone()),
    )
    .unwrap();
    assert!(matches!(
        binding_a.accept_local(cross_stream).unwrap(),
        ChannelAccept::Rejected(Reject::InvalidPairing)
    ));
    let (mut cross_peer_b, cross_stream_b) = UnixStream::pair().unwrap();
    write_record(
        &mut cross_peer_b,
        &control_hello(&hello_b, data_a.challenge.clone()),
    )
    .unwrap();
    assert!(matches!(
        binding_b.accept_local(cross_stream_b).unwrap(),
        ChannelAccept::Rejected(Reject::InvalidPairing)
    ));

    // Each honest pair still commits — a failed cross-wire consumed
    // nothing.
    let _connection_a = pair_control_half(&binding_a, &hello_a, &data_a.challenge);
    let _connection_b = pair_control_half(&binding_b, &hello_b, &data_b.challenge);
    assert_eq!(binding_a.status().phase, BindingPhase::Live);
    assert_eq!(binding_b.status().phase, BindingPhase::Live);
}

/// The delivery-window race: a correct-nonce `ControlHello` can only
/// have learned the challenge from the wire reply, yet thread
/// scheduling lets it arrive before `store_pending_channel` runs.
/// The verifier must wait — bounded by the pairing budget, the state
/// lock released — and commit once the store lands, never refuse an
/// honest pair for winning its own race.
#[test]
fn a_verified_control_hello_waits_for_the_in_flight_channel_store() {
    let (binding, hello) = reserved(required());
    let deadline = Instant::now() + Duration::from_secs(10);
    let (nonce, attempt, _admission) = binding
        .allocate_pending(&hello, ConnectionId::new("data-1"), deadline)
        .unwrap()
        .expect("the honest data Hello opens an attempt");
    assert!(binding.status().pending);

    // The data channel is deliberately unstored — the exact window
    // between the challenge reply hitting the wire and the accept
    // storing the stream.
    let (data_peer, data_stream) = UnixStream::pair().unwrap();
    let data_channel = RecordChannel::unix(data_stream);
    let data_shutdown = data_channel.shutdown_handle().unwrap();
    let (control_peer, control_stream) = UnixStream::pair().unwrap();
    let control_channel = RecordChannel::unix(control_stream);
    let control_shutdown = control_channel.shutdown_handle().unwrap();
    let PeerHello::Control(control_hello) = control_hello(&hello, nonce) else {
        panic!("control_hello builds a control record");
    };
    let shared = Arc::new(ControlShared::new());
    let handle = ControlHandle { shared };
    let (committed_tx, committed_rx) = mpsc::channel();
    thread::scope(|scope| {
        let committer = scope.spawn(|| {
            let commit = binding.commit_pair(
                &control_hello,
                ConnectionId::new("control-1"),
                control_shutdown,
                handle,
                Instant::now() + Duration::from_secs(30),
            );
            committed_tx.send(()).unwrap();
            commit
        });
        // The verified lane is parked waiting for the store — a
        // prompt refusal would have resolved already.
        assert_eq!(
            committed_rx.recv_timeout(Duration::from_millis(200)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "a verified nonce waits for the channel store"
        );
        // The store lands: the parked verifier wakes and commits.
        assert!(binding.store_pending_channel(attempt, data_channel, data_shutdown));
        let commit = committer.join().unwrap().expect("the honest pair commits");
        // The attempt's own deadline carries through — the control
        // lane's longer armed deadline cannot stretch it.
        assert_eq!(commit.deadline, deadline);
    });
    drop(data_peer);
    drop(control_peer);
    assert_eq!(binding.status().phase, BindingPhase::Live);
    binding.fence().shutdown();
}

/// A second authenticated data Hello while a pairing attempt is
/// alive is refused — never a supersession — so the honest pair's
/// pending half cannot be evicted by a parallel claim, and at most
/// one attempt (and one watchdog) exists per Session.
#[test]
fn a_second_data_hello_cannot_evict_a_live_pending_attempt() {
    let (binding, hello) = reserved(required());
    let (mut first, first_stream) = UnixStream::pair().unwrap();
    write_record(&mut first, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(first_stream).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut first).unwrap();
    let Ok(data) = reply else {
        panic!("the first data Hello is admitted");
    };

    // A second well-formed data claim: typed refusal on the wire,
    // the live attempt untouched.
    let (mut second, second_stream) = UnixStream::pair().unwrap();
    write_record(&mut second, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(second_stream).unwrap(),
        ChannelAccept::Rejected(Reject::AlreadyConnected)
    ));
    let reply: DataHelloReply = read_record(&mut second).unwrap();
    assert!(matches!(reply, Err(Reject::AlreadyConnected)));
    assert_eq!(binding.status().phase, BindingPhase::Reserved);

    // The original attempt still pairs — the failed claim consumed
    // nothing.
    let _connection = pair_control_half(&binding, &hello, &data.challenge);
    assert_eq!(binding.status().phase, BindingPhase::Live);
    binding.fence().shutdown();
}

/// One absolute budget spans both lanes: a control half that arrives
/// after the data lane's armed deadline is rejected as `InvalidPairing`
/// — its own freshly armed accept deadline can never extend the
/// pairing window.
#[test]
fn a_delayed_control_lane_cannot_extend_the_pairing_budget() {
    let (binding, hello) = reserved(required());
    let (mut peer, stream) = UnixStream::pair().unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    // A deliberately short armed budget on the data accept.
    assert!(matches!(
        binding
            .accept_local_deadline(stream, Instant::now() + Duration::from_millis(150))
            .unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer).unwrap();
    let Ok(data) = reply else {
        panic!("the honest data Hello is admitted");
    };

    // The control half arrives armed with a generous deadline of
    // its own — but the pairing budget already expired.
    std::thread::sleep(Duration::from_millis(250));
    let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
    write_record(
        &mut control_peer,
        &control_hello(&hello, data.challenge.clone()),
    )
    .unwrap();
    assert!(matches!(
        binding
            .accept_local_deadline(control_stream, Instant::now() + Duration::from_secs(30))
            .unwrap(),
        ChannelAccept::Rejected(Reject::InvalidPairing)
    ));
    let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
    assert!(matches!(reply, Err(Reject::InvalidPairing)));
    assert_eq!(binding.status().phase, BindingPhase::Reserved);
    assert!(!binding.status().pending);
}

#[test]
fn a_lost_control_lane_detaches_the_pair_and_blocks_new_dispatch() {
    let (binding, hello) = reserved(BTreeSet::from([Capability::Connection]));
    let (mut peer, stream) = UnixStream::pair().unwrap();
    write_record(&mut peer, &PeerHello::Data(hello.clone())).unwrap();
    assert!(matches!(
        binding.accept_local(stream).unwrap(),
        ChannelAccept::Pending
    ));
    let reply: DataHelloReply = read_record(&mut peer).unwrap();
    let Ok(data) = reply else {
        panic!("the honest data Hello is admitted");
    };

    // Pair the control lane and keep the peer end — no poll loop,
    // so dropping it is a genuine mid-life lane loss.
    let (mut control_peer, control_stream) = UnixStream::pair().unwrap();
    write_record(
        &mut control_peer,
        &control_hello(&hello, data.challenge.clone()),
    )
    .unwrap();
    let accepted = binding.accept_local(control_stream).unwrap();
    let ChannelAccept::Live(mut connection) = accepted else {
        panic!("the paired control lane commits the connection");
    };
    let reply: ControlHelloReply = read_record(&mut control_peer).unwrap();
    assert!(reply.is_ok());
    assert_eq!(binding.status().phase, BindingPhase::Live);

    // The control peer vanishes: the worker's lane ends, the pair
    // detaches, and no dispatch may start — the data socket is
    // interrupted, never silently retained.
    drop(control_peer);
    let deadline = Instant::now() + Duration::from_secs(10);
    while binding.status().phase == BindingPhase::Live {
        assert!(
            Instant::now() < deadline,
            "the lost lane never detached the pair"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(binding.status().phase, BindingPhase::Detached);
    assert!(connection.ping().is_err() || binding.control().is_none());
    drop(connection);
}
