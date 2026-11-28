//! `rustx-execution-client` — the physical Execution Client.
//!
//! The Client owns the physical Workspace, filesystem and search operations,
//! subprocesses, Managed Python, stdio MCP, and temporary output for one
//! authenticated Session binding. It connects over Unix sockets or WSS.
//!
//! The workspace is local policy, never a server request: `--workspace`
//! must match the bootstrap workspace byte for byte or the client refuses
//! to connect at all. Secrets never appear in argv or the environment: the
//! credential exists only inside the `0600` bootstrap file; on a Unix
//! endpoint the peer's euid is verified before it is sent, and on a `wss`
//! endpoint certificate and hostname validation complete first — there is
//! no flag that weakens either. An operator private CA enters only through
//! `--tls-ca-file`. Every error is a static message; record payloads are
//! never echoed.

use base64::Engine as _;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{env, fs};

use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::transport::{
    LOCAL_HANDSHAKE_TIMEOUT, PAGE_REQUEST_TIMEOUT, encode_record, read_record,
    recoverable_lane_loss,
};
use rustx_execution_protocol::{
    Admission, Bootstrap, BootstrapEndpoint, Capability, ClientReply, ClientRequest, ControlHello,
    ControlHelloReply, ControlPoll, ControlReply, ControlRequest, ControlTurn, DataHelloReply,
    DecimalU64, EntryKind, EntryStamp, FileStamp, Hello, LineEnd, MAX_DIRECTORY_NAMES,
    MAX_DIRECTORY_PAGE_ENTRIES, MAX_DOCUMENT_LOGICAL_BYTES, MAX_DOCUMENT_SOURCE_BYTES,
    MAX_RAW_CHUNK_BYTES, MAX_TEXT_BYTES, MAX_TEXT_LINES, PROTOCOL_VERSION, PageEntry, PairingNonce,
    PeerHello, ReadError, Reject, is_normalized_absolute_path,
    transport::{CONTROL_FRAME_TIMEOUT, CONTROL_POLL_INTERVAL},
};
use rustx_execution_transport::{ExecutionUrl, RecordChannel};

mod bash_capture;
mod changes;
mod managed_python;
mod mcp;
mod mutation;
mod process;
mod saves;
mod search;
mod skill_transfer;
mod skills;
mod terminal;
mod tree;
mod watch;

// The document decoder's policy constants and the wire's must be the
// same values — the decoder crate cannot depend on the protocol crate,
// so the build proves the identity here where both are visible.
const _: () = {
    assert!(
        rustx_document_reader::MAX_DOCUMENT_SOURCE_BYTES == MAX_DOCUMENT_SOURCE_BYTES,
        "document source bound must equal the wire policy"
    );
    assert!(
        rustx_document_reader::MAX_PROJECTED_DOCUMENT_BYTES as u64 == MAX_DOCUMENT_LOGICAL_BYTES,
        "document projection bound must equal the wire policy"
    );
};

/// The Client's locally configured home, before resolving User-tree aliases.
pub(crate) fn client_home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| is_normalized_absolute_path(home))
        .or_else(|| {
            nix::unistd::User::from_uid(nix::unistd::geteuid())
                .ok()
                .flatten()
                .map(|user| user.dir)
        })
        .filter(|home| is_normalized_absolute_path(home))
}

/// Resolve only locally selected HOME/`rustx` aliases. Published paths use a
/// physical spelling which the Client's no-follow file tools can open.
fn resolve_user_root(home: &Path) -> io::Result<PathBuf> {
    let configured = home.join("rustx");
    for prefix in configured.ancestors() {
        match fs::symlink_metadata(prefix) {
            Ok(_) => {
                // A dangling link is an existing entry. Canonicalization must
                // succeed before any missing descendants can be appended.
                let mut physical = prefix.canonicalize()?;
                if !physical.is_dir() {
                    return Err(io::ErrorKind::NotADirectory.into());
                }
                physical.extend(
                    configured
                        .strip_prefix(prefix)
                        .map_err(io::Error::other)?
                        .iter(),
                );
                return Ok(physical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::NotFound.into())
}

/// Official User tree in this Client's filesystem namespace.
pub(crate) fn client_user_root() -> Option<PathBuf> {
    client_home_directory()
        .and_then(|home| resolve_user_root(&home).ok())
        .filter(|root| is_normalized_absolute_path(root))
}

fn main() -> ExitCode {
    const USAGE: &str =
        "usage: rustx-client --binding-file PATH --workspace ABSOLUTE_PATH [--tls-ca-file PATH]";
    let mut args = env::args_os();
    let _program = args.next();
    let usage = || {
        eprintln!("{USAGE}");
        ExitCode::from(2)
    };
    let Some(binding_flag) = args.next() else {
        return usage();
    };
    if binding_flag == "--help" && args.len() == 0 {
        println!(
            "{USAGE}\nConnect to the Server using its private binding file. The Workspace must match the authorized path."
        );
        return ExitCode::SUCCESS;
    }
    if binding_flag == "--version" && args.len() == 0 {
        println!(
            "rustX Client {} (Execution Protocol v{PROTOCOL_VERSION})",
            env!("CARGO_PKG_VERSION")
        );
        return ExitCode::SUCCESS;
    }
    let Some(binding_path) = args.next() else {
        return usage();
    };
    let Some(workspace_flag) = args.next() else {
        return usage();
    };
    let Some(workspace_path) = args.next() else {
        return usage();
    };
    if binding_flag != OsStr::new("--binding-file") || workspace_flag != OsStr::new("--workspace") {
        return usage();
    }
    let ca_file = match (args.next(), args.next(), args.next()) {
        (None, None, None) => None,
        (Some(flag), Some(path), None) if flag == OsStr::new("--tls-ca-file") => {
            Some(PathBuf::from(path))
        }
        _ => return usage(),
    };
    match run(
        &PathBuf::from(binding_path),
        &PathBuf::from(workspace_path),
        ca_file.as_deref(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rustx-execution-client: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(binding_file: &Path, workspace: &Path, ca_file: Option<&Path>) -> Result<(), String> {
    // The credential endpoint and the socket endpoint must both live under
    // owner-only directories, or either could have been planted — and each
    // check runs against the pinned no-follow parent descriptor, never a
    // re-resolved pathname.
    let binding_parent = require_private_parent(binding_file)?;
    let mut file = open_private_file(&binding_parent, file_name(binding_file)?)?;
    let bootstrap: Bootstrap = read_record(&mut file).map_err(|_| invalid())?;
    // The file is exactly one record; trailing bytes are a corruption.
    match file.read(&mut [0u8; 1]) {
        Ok(0) => {}
        _ => return Err(invalid()),
    }
    // The endpoint is validated before any connect or credential
    // transmission: the Unix socket gets the same owner-only path checks
    // as before, and the WebSocket URL must satisfy the strict numeric-IP
    // numeric-IP TLS policy. A directly provisioned identity is the
    // only trust anchor for the loopback listener.
    let endpoint = match &bootstrap.endpoint {
        BootstrapEndpoint::Unix { socket_path } => {
            if ca_file.is_some() {
                return Err(String::from("untrusted endpoint"));
            }
            let socket_parent = require_private_parent(socket_path)?;
            require_socket(&socket_parent, file_name(socket_path)?)?;
            let reactor = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .map_err(|error| handshake_error(&error))?;
            Endpoint::Unix {
                path: socket_path.clone(),
                reactor,
            }
        }
        BootstrapEndpoint::WebSocket {
            url,
            certificate_der,
        } => {
            let validated =
                ExecutionUrl::validate(url).map_err(|_| String::from("untrusted endpoint"))?;
            if !validated.is_secure() {
                return Err(String::from("untrusted endpoint"));
            }
            let certificate = certificate_der
                .as_ref()
                .map(|encoded| {
                    if encoded.len() > 4096
                        || ca_file.is_some()
                        || !validated.address().ip().is_loopback()
                    {
                        return Err(String::from("untrusted endpoint"));
                    }
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|_| String::from("untrusted endpoint"))
                })
                .transpose()?;
            Endpoint::WebSocket(validated, certificate)
        }
    };

    // A bundle naming a protocol this build does not speak is refused
    // before any workspace or socket work.
    if bootstrap.version != PROTOCOL_VERSION {
        return Err(String::from("unsupported protocol version"));
    }

    // The workspace is local policy: the caller's `--workspace` must be the
    // bootstrap's exact path byte for byte — the client never silently
    // follows a path the server named. Fail closed before connecting.
    if !is_normalized_absolute_path(&bootstrap.workspace)
        || workspace.as_os_str() != bootstrap.workspace.as_os_str()
    {
        return Err(String::from("untrusted workspace"));
    }
    // The workspace root is opened once, no-follow component by component,
    // and held for the whole session; ancestor identities are re-verified
    // on every read.
    let root = PackageDirectory::open(&bootstrap.workspace)
        .map_err(|_| String::from("untrusted workspace"))?;

    let mut hello = Hello {
        version: PROTOCOL_VERSION,
        session: bootstrap.session,
        environment: bootstrap.environment,
        instance: bootstrap.instance,
        generation: bootstrap.generation,
        workspace: bootstrap.workspace,
        user_root: client_user_root().ok_or("Client User directory unavailable")?,
        // Independently known, fixed, actually implemented capabilities —
        // never read from the file.
        capabilities: BTreeSet::from([
            Capability::Connection,
            Capability::ReadRange,
            Capability::DirectoryMetadata,
            Capability::TreeMutations,
            Capability::FileWatches,
            Capability::WorkspaceChanges,
            Capability::WorkspacePython,
            Capability::ConditionalWrite,
            Capability::ConditionalEdit,
            Capability::ReadLines,
            Capability::DocumentPages,
            Capability::BoundedSearch,
            Capability::Process,
            Capability::AgentBash,
            Capability::ManagedOutput,
            Capability::StdioMcp,
            Capability::ManagedPython,
            Capability::UserSkills,
        ]),
        credential: bootstrap.credential,
    };
    let pair = pair_endpoint(
        &endpoint,
        &hello,
        Instant::now() + LOCAL_HANDSHAKE_TIMEOUT,
        ca_file,
    )
    .map_err(PairFailure::diagnostic)?;
    hello.credential = hello.credential.recovery_credential(&pair.challenge);
    let reconnect = |deadline| pair_endpoint(&endpoint, &hello, deadline, ca_file);
    serve_session(pair, &root, reconnect)
}

struct Pair {
    data: RecordChannel,
    control: RecordChannel,
    challenge: PairingNonce,
    admission: Admission,
}

#[derive(Debug)]
enum PairFailure {
    Transport(io::Error),
    Refused(Reject),
    Authentication(String),
}

impl PairFailure {
    fn diagnostic(self) -> String {
        match self {
            Self::Transport(error) => handshake_error(&error),
            Self::Refused(reject) => format!("rejected: {reject}"),
            Self::Authentication(reason) => reason,
        }
    }

    fn retryable(&self) -> bool {
        match self {
            Self::Transport(error) => {
                recoverable_lane_loss(error)
                    || matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    )
            }
            Self::Refused(Reject::AlreadyConnected) => true,
            _ => false,
        }
    }
}

impl From<io::Error> for PairFailure {
    fn from(error: io::Error) -> Self {
        Self::Transport(error)
    }
}

fn pair_endpoint(
    endpoint: &Endpoint,
    hello: &Hello,
    deadline: Instant,
    ca_file: Option<&Path>,
) -> Result<Pair, PairFailure> {
    let mut data = connect_endpoint(endpoint, deadline, ca_file)?;
    let encoded = encode_record(&PeerHello::Data(hello.clone()))?;
    data.send(&encoded, Some(deadline))?;
    let reply: DataHelloReply = data.receive(Some(deadline))?;
    let success = reply.map_err(PairFailure::Refused)?;
    verify_admission(hello, &success.admission).map_err(PairFailure::Authentication)?;
    let control = pair_control(endpoint, deadline, hello, &success, ca_file)?;
    Ok(Pair {
        data,
        control,
        challenge: success.challenge,
        admission: success.admission,
    })
}

fn recover_pair(
    reconnect: &impl Fn(Instant) -> Result<Pair, PairFailure>,
    admission: &Admission,
    cutoff: Instant,
) -> Result<Option<Pair>, String> {
    let mut pause = Duration::from_millis(100);
    loop {
        let now = Instant::now();
        if now >= cutoff {
            return Err(String::from("same-instance recovery expired"));
        }
        match reconnect(cutoff.min(now + LOCAL_HANDSHAKE_TIMEOUT)) {
            Ok(pair) if pair.admission == *admission => return Ok(Some(pair)),
            Ok(_) => {
                return Err(String::from(
                    "same-instance recovery changed binding identity",
                ));
            }
            Err(error) if error.retryable() => {
                std::thread::sleep(pause.min(cutoff.saturating_duration_since(Instant::now())));
                pause = (pause * 2).min(Duration::from_secs(1));
            }
            Err(PairFailure::Refused(Reject::Revoked)) => return Ok(None),
            Err(error) => return Err(error.diagnostic()),
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one Client binding and lane-recovery lifecycle"
)]
fn serve_session(
    mut pair: Pair,
    root: &PackageDirectory,
    reconnect: impl Fn(Instant) -> Result<Pair, PairFailure>,
) -> Result<(), String> {
    let admission = pair.admission.clone();
    let saves = Arc::new(Mutex::new(saves::Saves::new(admission.clone())));
    let operations = Arc::new(
        process::Operations::new(admission.clone()).map_err(|error| handshake_error(&error))?,
    );
    let result = std::thread::scope(|scope| {
        let (stop, retired) = std::sync::mpsc::channel();
        let registry = &saves;
        let maintenance = std::thread::Builder::new()
            .name("execution-save-cleanup".to_owned())
            .spawn_scoped(scope, move || saves::Saves::maintain(registry, &retired));
        let result = match maintenance {
            Ok(_) => {
                let mut state = DataState {
                    mcp: mcp::Sessions::default(),
                    python: managed_python::Catalog::new(&admission.workspace),
                    changes: changes::Changes::default(),
                    workspace_observation: changes::WorkspaceObservation::default(),
                    skills: skills::UserSkills::from_environment(),
                    next_ticket: None,
                };
                state.workspace_observation.poll(&operations);
                let revocation = Arc::new(ClientRevocation::default());
                let mut recovery_cutoff = None;
                let mut watches = Arc::new(watch::Watches::new());
                loop {
                    let arbitration = Arc::new(mutation::Arbitration::new());
                    let shutdown = pair
                        .data
                        .shutdown_handle()
                        .map_err(|error| handshake_error(&error))?;
                    let control = client_control_worker(
                        pair.control,
                        arbitration.clone(),
                        shutdown,
                        operations.clone(),
                        watches.clone(),
                        saves.clone(),
                        state.python.cleanup(),
                        revocation.clone(),
                    );
                    let outcome = serve(
                        &mut pair.data,
                        root,
                        &arbitration,
                        &operations,
                        &admission.workspace,
                        &watches,
                        &saves,
                        &mut state,
                        &revocation,
                    );
                    let now = Instant::now();
                    let cutoff = now + rustx_execution_protocol::DETACHED_CLIENT_RETENTION;
                    let control_end = control.stop();
                    pair.data.shutdown();
                    if !revocation.started() {
                        if !watches.shutdown() {
                            break Err(String::from("unresolved watch cleanup"));
                        }
                        watches = Arc::new(watch::Watches::new());
                    }
                    if control_end == ControlEnd::Revoked {
                        break Ok(());
                    }
                    if control_end == ControlEnd::Invalid
                        || !outcome.as_ref().is_err_and(recoverable_lane_loss)
                    {
                        break Err(String::from("execution pair violated the protocol"));
                    }
                    state
                        .skills
                        .lane_lost()
                        .map_err(|_| String::from("unresolved User Skill transfer cleanup"))?;
                    let cutoff = if revocation.started() {
                        *recovery_cutoff.get_or_insert(cutoff)
                    } else {
                        recovery_cutoff = None;
                        cutoff
                    };
                    match recover_pair(&reconnect, &admission, cutoff) {
                        Ok(Some(recovered)) => pair = recovered,
                        Ok(None) => break Ok(()),
                        Err(error) => break Err(error),
                    }
                }
            }
            Err(error) => Err(handshake_error(&error)),
        };
        drop(stop);
        result
    });
    // An unresolved worker is never reported as a clean exit.
    let operations_settled = operations.shutdown();
    if !saves.lock().expect("managed output").close() {
        return Err(String::from("unresolved managed output cleanup"));
    }
    let drained = operations_settled
        .and_then(|()| rustx_execution_client::workspace_python::cleanup_process_scratch());
    match (result, drained) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(format!("unresolved teardown: {error}")),
    }
}

/// The validated connection target derived from the bootstrap endpoint.
enum Endpoint {
    /// Owner-only Unix socket for the local sibling composition.
    Unix {
        path: PathBuf,
        reactor: tokio::runtime::Runtime,
    },
    /// Strictly validated WebSocket URL for the remote composition.
    WebSocket(ExecutionUrl, Option<Vec<u8>>),
}

/// Connects one lane to the validated endpoint — the same transport and
/// peer-owner proof for the data and control lanes alike.
fn connect_endpoint(
    endpoint: &Endpoint,
    deadline: std::time::Instant,
    ca_file: Option<&Path>,
) -> Result<RecordChannel, PairFailure> {
    match endpoint {
        Endpoint::Unix { path, reactor } => {
            let stream = reactor.block_on(async {
                tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    tokio::net::UnixStream::connect(path),
                )
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
            });
            let stream = match stream {
                Ok(stream) => stream.into_std()?,
                // A real local listener retains its exclusive socket for
                // the binding's lifetime. Its disappearance or refusal
                // means there is no local reattachment rendezvous.
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    return Err(PairFailure::Authentication(String::from(
                        "local execution listener is gone",
                    )));
                }
                Err(error) => return Err(error.into()),
            };
            stream.set_nonblocking(false)?;
            // The credential is sent only to a peer owned by this same
            // euid; a socket planted by another user never sees it.
            require_peer_owner(&stream).map_err(PairFailure::Authentication)?;
            Ok(RecordChannel::unix(stream))
        }
        Endpoint::WebSocket(endpoint, certificate) => {
            let channel = if let Some(certificate) = certificate {
                rustx_execution_transport::connect_provisioned(endpoint, deadline, certificate)
            } else {
                rustx_execution_transport::connect(endpoint, deadline, ca_file)
            };
            Ok(channel?)
        }
    }
}

/// Pairs the mandatory control lane: a second physical connection to the
/// same endpoint carries `ControlHello` with the `challenge` the Server
/// issued to the data half — the proof both lanes belong to one Client.
/// A committed pair replies with the same `Admission`; anything else
/// fails closed.
fn pair_control(
    endpoint: &Endpoint,
    deadline: std::time::Instant,
    data_hello: &Hello,
    data: &rustx_execution_protocol::DataAdmission,
    ca_file: Option<&Path>,
) -> Result<RecordChannel, PairFailure> {
    let mut channel = connect_endpoint(endpoint, deadline, ca_file)?;
    let hello = ControlHello {
        version: PROTOCOL_VERSION,
        session: data_hello.session.clone(),
        environment: data_hello.environment.clone(),
        instance: data_hello.instance.clone(),
        generation: data_hello.generation,
        workspace: data_hello.workspace.clone(),
        credential: data_hello.credential.clone(),
        connection_nonce: data.challenge.clone(),
    };
    let encoded = encode_record(&PeerHello::Control(hello))?;
    channel.send(&encoded, Some(deadline))?;
    let reply: ControlHelloReply = channel.receive(Some(deadline))?;
    let admission = reply.map_err(PairFailure::Refused)?;
    if admission != data.admission {
        return Err(PairFailure::Authentication(String::from(
            "control admission did not match data",
        )));
    }
    Ok(channel)
}

/// The joined control-lane worker; `stop` requests teardown and joins
/// it, so `run` never exits with a lane still live.
struct ClientControlWorker {
    /// The arbitration the worker shares with `serve` — `stop_worker`
    /// ends its loop.
    arbitration: std::sync::Arc<mutation::Arbitration>,
    join: std::thread::JoinHandle<ControlEnd>,
}

/// One admitted instance's terminal cut survives authenticated re-pairing.
#[derive(Default)]
struct ClientRevocation(AtomicU8);

impl ClientRevocation {
    const CLOSING: u8 = 1;
    const SETTLED: u8 = 2;

    fn started(&self) -> bool {
        self.0.load(Ordering::Acquire) >= Self::CLOSING
    }

    fn settled(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::SETTLED
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlEnd {
    Detached,
    Revoked,
    Invalid,
}

impl ClientControlWorker {
    fn stop(self) -> ControlEnd {
        self.arbitration.stop_worker();
        self.join.join().unwrap_or(ControlEnd::Invalid)
    }
}

/// The RAII fence the control worker drops on every exit path — a
/// protocol violation, EOF, or the orderly stop: local commits are
/// fenced and the data lane's physical stream is interrupted so a dead
/// control lane never leaves dispatch or a staged publish running
/// unarbitrated. Invoking the duplicated handle never touches the data
/// lock, so this resolves even while an exchange is blocked mid-write.
struct LaneFence {
    arbitration: std::sync::Arc<mutation::Arbitration>,
    data_shutdown: rustx_execution_transport::ChannelShutdown,
}

impl Drop for LaneFence {
    fn drop(&mut self) {
        self.arbitration.revoke();
        self.data_shutdown.shutdown();
    }
}

/// Spawns the control-lane worker: the single-owner turn loop on the
/// paired channel. The Client paces the lane — each `ControlPoll`
/// carries a fresh health nonce and the receipt for the previously
/// delivered request; the Server's `ControlTurn` must echo both before
/// its optional request is honored. `Health` echoes the Server's nonce,
/// `CancelMutation` arbitrates against the per-ticket cells, and
/// `Revoke` fences every local commit and acknowledges `Revoked` on one
/// final poll. When no request is outstanding the worker paces the next
/// poll by a bounded period — the lane never busy-spins, and every
/// frame runs under an absolute deadline.
fn settle_revocation(
    arbitration: &mutation::Arbitration,
    operations: &process::Operations,
    watches: &watch::Watches,
    saves: &std::sync::Arc<std::sync::Mutex<saves::Saves>>,
    python: &managed_python::SourceCleanup,
) -> bool {
    // Leave time in the Server's command budget for the physical receipt.
    let cutoff = std::time::Instant::now() + std::time::Duration::from_secs(4);
    arbitration.revoke();
    operations.revoke();
    let watches_settled = watches.shutdown();
    let remaining = cutoff.saturating_duration_since(std::time::Instant::now());
    let processes_settled = operations.finish_revocation(remaining);
    let python_settled = python.close();
    let saves_settled = saves::Saves::close_by(saves, cutoff);
    let settled = watches_settled && processes_settled.is_ok() && python_settled && saves_settled;
    #[cfg(target_os = "linux")]
    let settled =
        settled && rustx_process_supervisor::client_domain::release_staging(cutoff).is_ok();
    settled
}

#[allow(clippy::too_many_arguments)]
fn client_control_worker(
    mut channel: RecordChannel,
    arbitration: std::sync::Arc<mutation::Arbitration>,
    data_shutdown: rustx_execution_transport::ChannelShutdown,
    operations: std::sync::Arc<process::Operations>,
    watches: std::sync::Arc<watch::Watches>,
    saves: std::sync::Arc<std::sync::Mutex<saves::Saves>>,
    python: managed_python::SourceCleanup,
    revocation: Arc<ClientRevocation>,
) -> ClientControlWorker {
    let worker_arbitration = arbitration.clone();
    let join = std::thread::Builder::new()
        .name("execution-control".to_owned())
        .spawn(move || {
            // Every exit — a bad turn echo, EOF, a failed send, or the
            // orderly stop — drops this guard: the local commit
            // arbitration is fenced and the data lane is physically
            // interrupted, so no dispatch or staged commit can outlive
            // the control lane that arbitrates it.
            let _fence = LaneFence {
                arbitration: worker_arbitration.clone(),
                data_shutdown,
            };
            let mut sequence = 1_u64;
            let mut receipt: Option<ControlReply> = None;
            loop {
                if worker_arbitration.worker_stopped() {
                    return ControlEnd::Detached;
                }
                let Some(health) = fresh_nonce() else {
                    return ControlEnd::Invalid;
                };
                let final_receipt = matches!(&receipt, Some(ControlReply::Revoked));
                let poll = ControlPoll {
                    sequence: DecimalU64::new(sequence),
                    receipt: receipt.take(),
                    health,
                };
                let Ok(encoded) = encode_record(&poll) else {
                    return ControlEnd::Invalid;
                };
                let deadline = std::time::Instant::now() + CONTROL_FRAME_TIMEOUT;
                if let Err(error) = channel.send(&encoded, Some(deadline)) {
                    return control_io_end(&error);
                }
                let turn: ControlTurn = match channel.receive(Some(deadline)) {
                    Ok(turn) => turn,
                    Err(error) => return control_io_end(&error),
                };
                // The turn must answer this exact poll — sequence and
                // the fresh nonce both echo — before its request counts.
                if turn.sequence != poll.sequence || turn.health != poll.health {
                    operations.revoke();
                    return ControlEnd::Invalid;
                }
                if final_receipt {
                    return if turn.request.is_none() {
                        ControlEnd::Revoked
                    } else {
                        ControlEnd::Invalid
                    };
                }
                sequence = sequence.saturating_add(1);
                let Some(request) = turn.request else {
                    // Idle lane: pace the next poll — the worker wake
                    // covers the stop flag and keeps the cadence bounded.
                    if worker_arbitration.wait_worker(CONTROL_POLL_INTERVAL) {
                        return ControlEnd::Detached;
                    }
                    continue;
                };
                let reply = match request {
                    ControlRequest::Health { nonce } => ControlReply::Healthy { nonce },
                    ControlRequest::CancelMutation { ticket } => ControlReply::MutationCancelled {
                        ticket,
                        disposition: worker_arbitration.cancel(ticket),
                    },
                    ControlRequest::Revoke => {
                        if !revocation.settled() {
                            revocation
                                .0
                                .store(ClientRevocation::CLOSING, Ordering::Release);
                            if !settle_revocation(
                                &worker_arbitration,
                                &operations,
                                &watches,
                                &saves,
                                &python,
                            ) {
                                return ControlEnd::Detached;
                            }
                            revocation
                                .0
                                .store(ClientRevocation::SETTLED, Ordering::Release);
                        }
                        ControlReply::Revoked
                    }
                    ControlRequest::StopWatch { id } => ControlReply::WatchStopped {
                        id,
                        settled: watches.stop(id),
                    },
                    ControlRequest::CancelProcess { ref key } => ControlReply::ProcessCancelled {
                        key: key.clone(),
                        disposition: operations.cancel(key),
                    },
                };
                receipt = Some(reply);
                // A delivered request earns its receipt promptly — the
                // very next poll, not the idle cadence.
            }
        })
        .expect("the bounded control worker spawns");
    ClientControlWorker { arbitration, join }
}

/// A fresh health nonce for one poll — OS randomness, never a counter
/// the peer could predict and replay.
fn control_io_end(error: &io::Error) -> ControlEnd {
    if recoverable_lane_loss(error) {
        ControlEnd::Detached
    } else {
        ControlEnd::Invalid
    }
}

fn fresh_nonce() -> Option<DecimalU64> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).ok()?;
    Some(DecimalU64::new(u64::from_le_bytes(bytes)))
}

/// The peer's euid must equal this process's euid — the socket's other end
/// is the Server's own accept, not a planted endpoint.
#[cfg(target_os = "linux")]
fn require_peer_owner(stream: &UnixStream) -> Result<(), String> {
    let credentials =
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
            .map_err(|_| String::from("untrusted peer"))?;
    (credentials.uid() == nix::unistd::geteuid().as_raw())
        .then_some(())
        .ok_or_else(|| String::from("untrusted peer"))
}

#[cfg(target_os = "macos")]
fn require_peer_owner(stream: &UnixStream) -> Result<(), String> {
    let (uid, _) = nix::unistd::getpeereid(stream).map_err(|_| String::from("untrusted peer"))?;
    (uid == nix::unistd::geteuid())
        .then_some(())
        .ok_or_else(|| String::from("untrusted peer"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn require_peer_owner(stream: &UnixStream) -> Result<(), String> {
    let _ = stream;
    Err("peer ownership verification is unavailable".into())
}

/// The final component of a private path — the only piece the pinned
/// parent descriptor ever opens.
fn file_name(path: &Path) -> Result<&OsStr, String> {
    path.file_name().ok_or_else(invalid)
}

/// Opens `name` below the pinned private parent: `O_RDONLY` with
/// `O_NOFOLLOW|O_CLOEXEC|O_NONBLOCK`, then fstat-proves a regular file
/// owned by this euid at `0600` — a FIFO, symlink, or world-readable
/// replacement is never trusted.
fn open_private_file(dir: &fs::File, name: &OsStr) -> Result<fs::File, String> {
    let file = nix::fcntl::openat(
        dir,
        name,
        nix::fcntl::OFlag::O_RDONLY
            | nix::fcntl::OFlag::O_NOFOLLOW
            | nix::fcntl::OFlag::O_CLOEXEC
            | nix::fcntl::OFlag::O_NONBLOCK,
        nix::sys::stat::Mode::empty(),
    )
    .map_err(|_| invalid())?;
    let file = fs::File::from(file);
    let metadata = file.metadata().map_err(|_| invalid())?;
    let euid = nix::unistd::geteuid().as_raw();
    if !metadata.file_type().is_file()
        || metadata.uid() != euid
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(invalid());
    }
    Ok(file)
}

/// Pins the path's parent directory: the whole component chain is
/// walked no-follow from `/` by [`rustx_execution_transport::open_dir_nofollow`], then the
/// descriptor itself is proven a directory owned by this euid at
/// `0700` — never a re-resolved pathname.
fn require_private_parent(path: &Path) -> Result<fs::File, String> {
    let parent = path.parent().ok_or_else(invalid)?;
    let dir = rustx_execution_transport::open_dir_nofollow(parent).map_err(|_| invalid())?;
    let metadata = dir.metadata().map_err(|_| invalid())?;
    let euid = nix::unistd::geteuid().as_raw();
    if !metadata.is_dir() || metadata.uid() != euid || metadata.mode() & 0o777 != 0o700 {
        return Err(invalid());
    }
    Ok(dir)
}

/// Proves `name` below the pinned parent is a socket owned by this
/// euid — via `fstatat(AT_SYMLINK_NOFOLLOW)`, never by following a
/// pathname.
fn require_socket(dir: &fs::File, name: &OsStr) -> Result<(), String> {
    let metadata = nix::sys::stat::fstatat(dir, name, nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|_| String::from("untrusted endpoint"))?;
    let kind = metadata.st_mode & nix::sys::stat::SFlag::S_IFMT.bits();
    (kind == nix::sys::stat::SFlag::S_IFSOCK.bits()
        && metadata.st_uid == nix::unistd::geteuid().as_raw())
    .then_some(())
    .ok_or_else(|| String::from("untrusted endpoint"))
}

/// The admission must echo the exact reservation the Hello claimed —
/// every identity field plus the workspace — or the peer is not the
/// Server that owns this binding.
fn verify_admission(hello: &Hello, admission: &Admission) -> Result<(), String> {
    (admission.version == hello.version
        && admission.session == hello.session
        && admission.environment == hello.environment
        && admission.instance == hello.instance
        && admission.generation == hello.generation
        && admission.workspace == hello.workspace)
        .then_some(())
        .ok_or_else(|| String::from("untrusted admission"))
}

struct DataState {
    mcp: mcp::Sessions,
    python: managed_python::Catalog,
    changes: changes::Changes,
    workspace_observation: changes::WorkspaceObservation,
    skills: skills::UserSkills,
    next_ticket: Option<u64>,
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // One typed data lane.
fn serve(
    channel: &mut RecordChannel,
    root: &PackageDirectory,
    arbitration: &Arc<mutation::Arbitration>,
    operations: &Arc<process::Operations>,
    workspace: &Path,
    watches: &watch::Watches,
    saves: &Mutex<saves::Saves>,
    state: &mut DataState,
    revocation: &ClientRevocation,
) -> io::Result<()> {
    let mut writes = mutation::Writes::new(arbitration.clone());
    if let Some(ticket) = state.next_ticket {
        writes.continue_ticket_sequence(ticket);
    }
    let outcome = serve_requests(
        channel,
        root,
        arbitration,
        operations,
        workspace,
        watches,
        saves,
        state,
        &mut writes,
        revocation,
    );
    state.next_ticket = Some(writes.ticket_sequence());
    outcome
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // The typed one-request/one-reply dispatch loop.
fn serve_requests(
    channel: &mut RecordChannel,
    root: &PackageDirectory,
    arbitration: &mutation::Arbitration,
    operations: &Arc<process::Operations>,
    workspace: &Path,
    watches: &watch::Watches,
    saves: &Mutex<saves::Saves>,
    state: &mut DataState,
    writes: &mut mutation::Writes,
    revocation: &ClientRevocation,
) -> io::Result<()> {
    loop {
        let request = channel.receive::<ClientRequest>(None)?;
        if revocation.started() {
            channel.send(
                &encode_record(&ClientReply::Error {
                    kind: ReadError::Unavailable,
                })?,
                Some(Instant::now() + PAGE_REQUEST_TIMEOUT),
            )?;
            continue;
        }
        let reply = match request {
            ClientRequest::Ping => ClientReply::Pong,
            ClientRequest::UserSkills(request) => {
                let inquiry = matches!(
                    request,
                    rustx_execution_protocol::skills::Request::Root { .. }
                        | rustx_execution_protocol::skills::Request::RootIdentity
                        | rustx_execution_protocol::skills::Request::Collection { .. }
                        | rustx_execution_protocol::skills::Request::Packages { .. }
                        | rustx_execution_protocol::skills::Request::Standalone { .. }
                        | rustx_execution_protocol::skills::Request::Digest { .. }
                );
                let transfer = if inquiry {
                    None
                } else {
                    operations.admit_auxiliary()
                };
                if inquiry || transfer.is_some() {
                    state.skills.reply(&request, arbitration)
                } else {
                    ClientReply::Error {
                        kind: ReadError::Unavailable,
                    }
                }
            }
            ClientRequest::WorkspacePythonStatus => {
                ClientReply::WorkspacePythonStatus(operations.workspace_python_status())
            }
            ClientRequest::PrepareWorkspacePython => {
                if operations.schedule_workspace_python() {
                    ClientReply::WorkspacePythonScheduled
                } else {
                    ClientReply::Error {
                        kind: ReadError::Unavailable,
                    }
                }
            }

            ClientRequest::Process(process_request) => {
                ClientReply::Process(operations.dispatch(process_request))
            }
            ClientRequest::ReadBashOutput {
                key,
                conversation,
                offset,
                limit,
            } => operations
                .with_bash_file(&key, &conversation, |file| {
                    file.read(|root, name| read_lines(root, name, offset, limit, None))
                })
                .flatten()
                .unwrap_or(ClientReply::Error {
                    kind: ReadError::NotFound,
                }),
            ClientRequest::SearchBashOutput {
                key,
                conversation,
                spec,
            } => {
                if matches!(spec, rustx_execution_protocol::SearchSpec::Grep { .. }) {
                    operations
                        .with_bash_file(&key, &conversation, |file| {
                            file.read(|root, name| {
                                search::search(
                                    root,
                                    name,
                                    &spec,
                                    std::time::Instant::now() + PAGE_REQUEST_TIMEOUT,
                                )
                            })
                        })
                        .flatten()
                        .unwrap_or(ClientReply::Error {
                            kind: ReadError::NotFound,
                        })
                } else {
                    ClientReply::Error {
                        kind: ReadError::Invalid,
                    }
                }
            }
            ClientRequest::Mcp(request) => {
                ClientReply::Mcp(state.mcp.exchange(request, operations, &state.python))
            }
            ClientRequest::PythonSource(request) => {
                ClientReply::PythonSource(if arbitration.is_revoked() {
                    rustx_execution_protocol::mcp::PythonSourceReply::Refused(
                        ReadError::Unavailable,
                    )
                } else {
                    state.python.exchange(request, operations)
                })
            }
            ClientRequest::Save(request) => ClientReply::Save(if arbitration.is_revoked() {
                rustx_execution_protocol::save::SaveReply::Refused {
                    code: rustx_execution_protocol::save::SaveCode::StaleOwner,
                }
            } else {
                saves
                    .lock()
                    .expect("managed output")
                    .dispatch(&request, std::time::Instant::now())
            }),
            ClientRequest::ReadSavedOutput {
                locator,
                conversation,
                offset,
                limit,
            } => saves
                .lock()
                .expect("managed output")
                .with_published(&locator, &conversation, |root, path| {
                    read_lines(root, path, offset, limit, None)
                })
                .unwrap_or_else(|error| ClientReply::Error {
                    kind: read_error(&error),
                }),
            ClientRequest::ReadSavedOutputRange {
                locator,
                conversation,
                offset,
            } => saves
                .lock()
                .expect("managed output")
                .with_published(&locator, &conversation, |root, path| {
                    read_range(
                        root,
                        path,
                        offset,
                        u32::try_from(rustx_execution_protocol::MAX_TEXT_BYTES)
                            .expect("bounded saved-output page"),
                        None,
                    )
                })
                .unwrap_or_else(|error| ClientReply::Error {
                    kind: read_error(&error),
                }),
            ClientRequest::SearchSavedOutput {
                locator,
                conversation,
                spec,
            } => {
                if matches!(spec, rustx_execution_protocol::SearchSpec::Grep { .. }) {
                    saves
                        .lock()
                        .expect("managed output")
                        .with_published(&locator, &conversation, |root, path| {
                            search::search_as(
                                root,
                                path,
                                Some(locator.file_name()),
                                &spec,
                                std::time::Instant::now() + PAGE_REQUEST_TIMEOUT,
                            )
                        })
                        .unwrap_or_else(|error| ClientReply::Error {
                            kind: read_error(&error),
                        })
                } else {
                    ClientReply::Error {
                        kind: ReadError::Invalid,
                    }
                }
            }
            ClientRequest::ReadRange {
                relative_path,
                offset,
                max_bytes,
                expected_revision,
            } => read_range(
                root,
                &relative_path,
                offset,
                max_bytes,
                expected_revision.as_deref(),
            ),
            ClientRequest::GetMetadata { relative_path } => get_metadata(root, &relative_path),
            ClientRequest::ResolveResourceEntry { relative_path } => {
                resolve_resource_entry(root, workspace, &relative_path)
            }
            ClientRequest::WorkspaceChanges(request) => ClientReply::WorkspaceChanges(
                state
                    .changes
                    .dispatch(operations, operations.workspace(), request),
            ),
            ClientRequest::MutateTree(request) => match operations.admit_auxiliary() {
                Some(_mutation) => tree::mutate(root, arbitration, &request),
                None => ClientReply::Error {
                    kind: ReadError::Unavailable,
                },
            },
            ClientRequest::Watch(request) => {
                let mut reply = watches.dispatch(root, workspace, request);
                if let rustx_execution_protocol::watch::WatchReply::Changed(change) = &mut reply
                    && change.id.is_configuration()
                {
                    change.resync_required |= state.workspace_observation.poll(operations);
                }
                ClientReply::Watch(reply)
            }
            ClientRequest::ReadDirectory {
                relative_path,
                offset,
                limit,
                expected_revision,
                skip_hidden,
            } => read_directory(
                root,
                &relative_path,
                offset,
                limit,
                expected_revision.as_deref(),
                skip_hidden,
            ),
            ClientRequest::PrepareWrite {
                relative_path,
                expectation,
                data_base64,
            } => writes.prepare(root, &relative_path, &expectation, &data_base64),
            ClientRequest::BeginWrite {
                relative_path,
                expectation,
                total,
            } => writes.begin(root, &relative_path, &expectation, total),
            ClientRequest::WriteChunk {
                ticket,
                offset,
                data_base64,
            } => writes.chunk(ticket, offset, &data_base64),
            ClientRequest::PrepareEdit {
                relative_path,
                expectation,
                replacements,
            } => writes.prepare_edit(root, &relative_path, &expectation, &replacements),
            ClientRequest::CommitWrite { ticket } => match operations.admit_auxiliary() {
                Some(_staging) => writes.commit(ticket),
                None => ClientReply::WriteCancelled { ticket },
            },
            ClientRequest::AbortWrite { ticket } => writes.abort(ticket),
            ClientRequest::ReadLines {
                relative_path,
                offset,
                limit,
                expected_revision,
            } => read_lines(
                root,
                &relative_path,
                offset,
                limit,
                expected_revision.as_deref(),
            ),
            ClientRequest::ReadDocument {
                relative_path,
                offset,
                limit,
                expected_revision,
            } => read_document(
                root,
                &relative_path,
                offset,
                limit,
                expected_revision.as_deref(),
            ),
            ClientRequest::Search {
                relative_root,
                spec,
            } => search::search(
                root,
                &relative_root,
                &spec,
                std::time::Instant::now() + PAGE_REQUEST_TIMEOUT,
            ),
        };
        let encoded = encode_record(&reply)?;
        channel.send(&encoded, None)?;
    }
}

/// Performs the one physical operation: `relative_path` must pass the
/// shared lexical gate and name a file — never the empty root —
/// `max_bytes` within `1..=`[`MAX_RAW_CHUNK_BYTES`], and the bytes
/// verified under the file's revision. Errors cross the wire as
/// stable [`ReadError`] codes only — never raw paths.
fn read_range(
    root: &PackageDirectory,
    relative_path: &Path,
    offset: DecimalU64,
    max_bytes: u32,
    expected_revision: Option<&str>,
) -> ClientReply {
    let bound = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    if !(1..=MAX_RAW_CHUNK_BYTES).contains(&bound)
        || relative_path.as_os_str().is_empty()
        || relative_entry(relative_path).is_err()
    {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    }
    match rustx_confined_fs::read_range(
        relative_path,
        offset.get(),
        bound,
        MAX_RAW_CHUNK_BYTES,
        expected_revision,
        |relative| root.file(relative),
    ) {
        Ok(chunk) => {
            // The wire file type is implicitly regular: the same stamp that
            // verified the read projects it — never a fresh stat.
            if !chunk.metadata.is_file {
                return ClientReply::Error {
                    kind: ReadError::Denied,
                };
            }
            ClientReply::Range {
                data_base64: chunk.data_base64,
                offset,
                next_offset: chunk.next_offset.map(DecimalU64::new),
                metadata: FileStamp {
                    size: DecimalU64::new(chunk.metadata.size),
                    revision: chunk.metadata.revision,
                    identity: chunk.metadata.identity,
                    modified_at_ms: chunk.metadata.modified_at_ms,
                },
            }
        }
        Err(error) => ClientReply::Error {
            kind: read_error(&error),
        },
    }
}

/// `ReadLines`: one bounded logical-text page of `relative_path` under the
/// workspace — the same streaming scanner the native Read tool projects,
/// under the same complete-line byte/line budgets. The file is opened
/// once, stamped before the scan, and proven unchanged after it (the
/// retained descriptor plus a freshly opened path) — a mid-scan change is
/// a conflict, never unverified text. Errors cross as stable [`ReadError`]
/// codes only; non-UTF-8 bytes anywhere in the source — the whole source
/// is validated even past the page — are [`ReadError::InvalidUtf8`].
fn read_lines(
    root: &PackageDirectory,
    relative_path: &Path,
    offset: DecimalU64,
    limit: Option<DecimalU64>,
    expected_revision: Option<&str>,
) -> ClientReply {
    // A text page names one file leaf — never the empty root — and the
    // scanner's contract rejects a zero offset or a zero requested limit.
    let Ok(relative) = relative_entry(relative_path) else {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    };
    if relative.as_os_str().is_empty()
        || offset.get() == 0
        || limit.is_some_and(|limit| limit.get() == 0)
    {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    }
    let file = match root.file(relative) {
        Ok(file) => file,
        Err(error) => {
            return ClientReply::Error {
                kind: read_error(&error),
            };
        }
    };
    let Ok(before) = rustx_confined_fs::FileMetadata::of(&file) else {
        return ClientReply::Error {
            kind: ReadError::Unavailable,
        };
    };
    if expected_revision.is_some_and(|expected| expected != before.revision) {
        return ClientReply::Error {
            kind: ReadError::Conflict,
        };
    }
    // The wire policy constants are the scanner's own bound — one policy,
    // no duplicate source. The scan is bounded to the stamped size: a
    // file that keeps growing can never stream without end, and the
    // scanned byte count must equal the stamp exactly.
    let limits = rustx_confined_fs::PageLimits {
        max_bytes: MAX_TEXT_BYTES,
        max_lines: MAX_TEXT_LINES,
    };
    let page = match limits.scan_text(
        (&file).take(before.size),
        offset.get(),
        limit.map(DecimalU64::get),
    ) {
        Ok(page) => page,
        Err(error) => {
            return ClientReply::Error {
                kind: if error.kind() == std::io::ErrorKind::InvalidData {
                    ReadError::InvalidUtf8
                } else {
                    read_error(&error)
                },
            };
        }
    };
    if page.original_bytes != before.size {
        return ClientReply::Error {
            kind: ReadError::Conflict,
        };
    }
    // The retained descriptor and a freshly opened path must still name
    // the file that was scanned.
    let unchanged = rustx_confined_fs::FileMetadata::of(&file).is_ok_and(|after| after == before)
        && root
            .file(relative)
            .and_then(|reopened| rustx_confined_fs::FileMetadata::of(&reopened))
            .is_ok_and(|reopened| reopened == before);
    if !unchanged {
        return ClientReply::Error {
            kind: ReadError::Conflict,
        };
    }
    ClientReply::Lines {
        metadata: stamp(&before).stamp,
        text: page.text,
        total_lines: DecimalU64::new(page.total_lines),
        shown_lines: page.shown_lines,
        end: line_end(page.end),
    }
}

/// The wire mirror of the scanner's page-end reason — one projection for
/// every logical-text reply.
fn line_end(end: rustx_confined_fs::PageEnd) -> LineEnd {
    match end {
        rustx_confined_fs::PageEnd::Complete => LineEnd::Complete,
        rustx_confined_fs::PageEnd::RequestedLimit => LineEnd::RequestedLimit,
        rustx_confined_fs::PageEnd::LineLimit => LineEnd::LineLimit,
        rustx_confined_fs::PageEnd::ByteLimit => LineEnd::ByteLimit,
        rustx_confined_fs::PageEnd::FirstLineTooLarge { bytes } => LineEnd::FirstLineTooLarge {
            bytes: DecimalU64::new(bytes),
        },
        rustx_confined_fs::PageEnd::OffsetPastEnd => LineEnd::OffsetPastEnd,
    }
}

/// `ReadDocument`: one bounded logical-text page of a whitelisted
/// structured document under the workspace — the same classification,
/// acquisition bound, and parser policy the native Read tool runs, served
/// through `rustx-document-reader`. The document is opened no-follow,
/// stamped before acquisition, read through the shared bounded read under
/// the stamped-size cap, decoded, and proven unchanged after (the
/// retained descriptor plus a freshly opened path) — a mid-decode change
/// is a conflict, never unverified text. `logical_bytes` counts the
/// decoded Markdown, independent of the compressed source's byte length.
/// Errors cross as stable [`ReadError`] codes only — never raw paths,
/// parser messages, or payload.
fn read_document(
    root: &PackageDirectory,
    relative_path: &Path,
    offset: DecimalU64,
    limit: Option<DecimalU64>,
    expected_revision: Option<&str>,
) -> ClientReply {
    // A document page names one file leaf — never the empty root — and
    // the scanner's contract rejects a zero offset or a zero requested
    // limit, all before any filesystem access.
    let Ok(relative) = relative_entry(relative_path) else {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    };
    if relative.as_os_str().is_empty()
        || offset.get() == 0
        || limit.is_some_and(|limit| limit.get() == 0)
    {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    }
    // The rustX whitelist picks the decoder by extension — a text source
    // has no document decode, and there is no content sniffing.
    let rustx_execution_protocol::SourceKind::Document(format) =
        rustx_execution_protocol::classify_source(relative)
    else {
        return ClientReply::Error {
            kind: ReadError::UnsupportedDocument,
        };
    };
    let (file, before, text) = match document_source(root, relative, format, expected_revision) {
        Ok(source) => source,
        Err(kind) => {
            return ClientReply::Error { kind };
        }
    };
    // The decoded Markdown pages under the same complete-line budgets as
    // a text source; the projection itself is a `String`, so the scan
    // cannot fail on UTF-8.
    let limits = rustx_confined_fs::PageLimits {
        max_bytes: MAX_TEXT_BYTES,
        max_lines: MAX_TEXT_LINES,
    };
    let page = match limits.scan_text(text.as_bytes(), offset.get(), limit.map(DecimalU64::get)) {
        Ok(page) => page,
        Err(error) => {
            return ClientReply::Error {
                kind: read_error(&error),
            };
        }
    };
    // The retained descriptor and a freshly opened path must still name
    // the document that was decoded: an unstable or corrupt source never
    // leaks a page.
    let unchanged = rustx_confined_fs::FileMetadata::of(&file).is_ok_and(|after| after == before)
        && root
            .file(relative)
            .and_then(|reopened| rustx_confined_fs::FileMetadata::of(&reopened))
            .is_ok_and(|reopened| reopened == before);
    if !unchanged {
        return ClientReply::Error {
            kind: ReadError::Conflict,
        };
    }
    ClientReply::DocumentPage {
        source: stamp(&before).stamp,
        logical_bytes: DecimalU64::new(page.original_bytes),
        text: page.text,
        total_lines: DecimalU64::new(page.total_lines),
        shown_lines: page.shown_lines,
        end: line_end(page.end),
    }
}

/// Opens the document under the retained no-follow descriptor, enforces
/// the caller's expected revision and the stamped-size source cap before
/// a single parser step runs, and returns the decoded Markdown with the
/// descriptor and the stamp the source must still match after decoding.
/// The acquisition is bounded twice: the stamped-size `take` keeps an
/// appended tail out of the decode, and the shared read keeps its own
/// limit+1 ceiling; the observed byte count must equal the stamp — a
/// shrink mid-read is a conflict, never a partial document.
fn document_source(
    root: &PackageDirectory,
    relative: &Path,
    format: rustx_execution_protocol::DocumentFormat,
    expected_revision: Option<&str>,
) -> Result<(fs::File, rustx_confined_fs::FileMetadata, String), ReadError> {
    let file = root.file(relative).map_err(|error| read_error(&error))?;
    let before = rustx_confined_fs::FileMetadata::of(&file).map_err(|_| ReadError::Unavailable)?;
    if expected_revision.is_some_and(|expected| expected != before.revision) {
        return Err(ReadError::Conflict);
    }
    // The source bound is a hard pre-decode cap on the stamped size: an
    // oversized document is refused before a single byte is read or a
    // single parser step runs.
    if before.size > MAX_DOCUMENT_SOURCE_BYTES as u64 {
        return Err(ReadError::DocumentTooLarge);
    }
    let bytes =
        rustx_document_reader::read_source((&file).take(before.size)).map_err(
            |error| match error {
                rustx_document_reader::SourceError::Io(error) => read_error(&error),
                rustx_document_reader::SourceError::OverLimit => ReadError::DocumentTooLarge,
            },
        )?;
    if bytes.len() as u64 != before.size {
        return Err(ReadError::Conflict);
    }
    let text = rustx_document_reader::decode(bytes, format).map_err(|error| match error {
        rustx_document_reader::DecodeError::SourceTooLarge => ReadError::DocumentTooLarge,
        rustx_document_reader::DecodeError::Runtime(_) => ReadError::Unavailable,
        rustx_document_reader::DecodeError::Failed(_) => ReadError::DocumentDecodeFailed,
        rustx_document_reader::DecodeError::NoTextLayer => ReadError::NoTextLayer,
    })?;
    Ok((file, before, text))
}

/// The shared physical-error projection: stable wire codes only, never a
/// raw path or payload. A symlink loop mid-path is a denial — the
/// traversal never follows one — and a component that is not a directory
/// makes the addressed entry absent.
fn read_error(error: &std::io::Error) -> ReadError {
    if error.raw_os_error() == Some(nix::libc::ELOOP) {
        return ReadError::Denied;
    }
    match error.kind() {
        std::io::ErrorKind::InvalidInput => ReadError::Invalid,
        std::io::ErrorKind::PermissionDenied => ReadError::Denied,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => ReadError::NotFound,
        std::io::ErrorKind::AlreadyExists => ReadError::Conflict,
        _ => ReadError::Unavailable,
    }
}

/// The lexical gate every physical request shares: normal components
/// only, never absolute. The raw spelling is checked, not `components()`
/// — an interior `.` or `..` segment that `components()` normalizes away
/// and a NUL byte are refused exactly like a leading `.`; every other
/// byte, control characters included, is ordinary literal name data on
/// both DTO sides. An empty path — legal for the query operations —
/// names the workspace root itself.
fn relative_entry(relative_path: &Path) -> Result<&Path, ReadError> {
    let raw = relative_path.as_os_str().as_bytes();
    if relative_path.is_absolute()
        || raw.contains(&0)
        || raw
            .split(|byte| *byte == b'/')
            .any(|segment| segment == b"." || segment == b"..")
    {
        return Err(ReadError::Invalid);
    }
    Ok(relative_path)
}

/// Projects one verified `rustx-confined-fs` stamp onto the wire — the
/// same `FileMetadata` the physical check produced, never a fresh stat.
fn stamp(metadata: &rustx_confined_fs::FileMetadata) -> EntryStamp {
    EntryStamp {
        kind: if metadata.is_file {
            EntryKind::RegularFile
        } else if metadata.is_directory {
            EntryKind::Directory
        } else if metadata.is_symlink {
            EntryKind::Symlink
        } else {
            EntryKind::Other
        },
        stamp: FileStamp {
            size: DecimalU64::new(metadata.size),
            revision: metadata.revision.clone(),
            identity: metadata.identity.clone(),
            modified_at_ms: metadata.modified_at_ms,
        },
    }
}

/// `GetMetadata`: `relative_path` names one entry — or the workspace root
/// itself when empty. Intermediate components resolve no-follow; a
/// symlinked leaf reports `symlink` and is never followed. Errors cross
/// as stable [`ReadError`] codes only.
fn get_metadata(root: &PackageDirectory, relative_path: &Path) -> ClientReply {
    let Ok(relative) = relative_entry(relative_path) else {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    };
    match root.entry_metadata(relative) {
        Ok(metadata) => ClientReply::Metadata {
            metadata: stamp(&metadata),
        },
        Err(error) => ClientReply::Error {
            kind: read_error(&error),
        },
    }
}

fn resolve_resource_entry(
    root: &PackageDirectory,
    workspace: &Path,
    relative_path: &Path,
) -> ClientReply {
    let resolution = match root.resolve_resource(workspace, relative_path) {
        Ok(resolution) => resolution,
        Err(error) => {
            return ClientReply::Error {
                kind: read_error(&error),
            };
        }
    };
    if resolution
        .link_paths
        .iter()
        .chain(resolution.unresolved_path.iter())
        .any(|path| path.to_str().is_none())
    {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    }
    match resolution.entry {
        Some(entry) if entry.relative_path.to_str().is_some() => ClientReply::ResourceEntry {
            relative_path: entry.relative_path,
            metadata: stamp(&entry.metadata),
            link_paths: resolution.link_paths,
        },
        Some(_) => ClientReply::Error {
            kind: ReadError::Invalid,
        },
        None => ClientReply::ResourceUnavailable {
            link_paths: resolution.link_paths,
            unresolved_path: resolution.unresolved_path,
        },
    }
}

/// `ReadDirectory`: one revision-checked page under the workspace — an
/// empty `relative_path` lists the root. Bounds are enforced before any
/// physical work; errors cross as stable [`ReadError`] codes only.
fn read_directory(
    root: &PackageDirectory,
    relative_path: &Path,
    offset: u32,
    limit: u32,
    expected_revision: Option<&str>,
    skip_hidden: bool,
) -> ClientReply {
    let Ok(relative) = relative_entry(relative_path) else {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    };
    let Ok(offset) = usize::try_from(offset) else {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    };
    let Ok(limit) = usize::try_from(limit) else {
        return ClientReply::Error {
            kind: ReadError::Invalid,
        };
    };
    let page = if skip_hidden {
        root.skill_directory_page(
            relative,
            offset,
            limit,
            expected_revision,
            MAX_DIRECTORY_NAMES,
            MAX_DIRECTORY_PAGE_ENTRIES,
        )
    } else {
        root.directory_page(
            relative,
            offset,
            limit,
            expected_revision,
            MAX_DIRECTORY_NAMES,
            MAX_DIRECTORY_PAGE_ENTRIES,
        )
        .map(|page| (page, 0))
    };
    match page {
        Ok((page, skipped)) => ClientReply::Directory {
            skipped: u32::try_from(skipped).expect("bounded directory page"),
            revision: page.revision,
            entries: page
                .entries
                .iter()
                .map(|entry| PageEntry {
                    name: entry.file_name.clone(),
                    metadata: stamp(&entry.metadata),
                })
                .collect(),
            next_offset: page
                .next_offset
                .and_then(|offset| u32::try_from(offset).ok()),
        },
        Err(error) => ClientReply::Error {
            kind: if error.kind() == std::io::ErrorKind::FileTooLarge {
                ReadError::DirectoryEntryLimit
            } else {
                read_error(&error)
            },
        },
    }
}

/// Handshake-phase failures are static, redacted messages; only the
/// deadline kind earns its own wording.
fn handshake_error(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::TimedOut => String::from("handshake timed out"),
        _ => String::from("handshake"),
    }
}

fn invalid() -> String {
    String::from("invalid binding file")
}
