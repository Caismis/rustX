//! `rustx-execution-client` — the physical Execution Client.
//!
//! This layer owns exactly one local Session binding: it reads a
//! server-published [`Bootstrap`] from an owner-only file, opens the bound
//! workspace as a [`PackageDirectory`] held by descriptor for the whole
//! session, performs the authenticated `Hello`/`Admission` handshake over
//! the bound Unix socket, then serves the Server's typed requests until
//! EOF: `Ping` liveness and bounded revision-checked `ReadRange` — the one
//! implemented physical operation, executed through `rustx-confined-fs`,
//! the same primitives the Agent Server itself uses. No process, MCP, or
//! Python I/O exists here.
//!
//! The workspace is local policy, never a server request: `--workspace`
//! must match the bootstrap workspace byte for byte or the client refuses
//! to connect at all. Secrets never appear in argv or the environment: the
//! credential exists only inside the `0600` bootstrap file, and the peer's
//! euid is verified before it is sent. Every error is a static message;
//! record payloads are never echoed.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use rustx_confined_fs::PackageDirectory;
use rustx_execution_protocol::transport::{
    Handshake, LOCAL_HANDSHAKE_TIMEOUT, read_optional_record, read_record, write_record,
};
use rustx_execution_protocol::{
    Admission, Bootstrap, Capability, ClientReply, ClientRequest, DecimalU64, Hello, HelloReply,
    MAX_RAW_CHUNK_BYTES, PROTOCOL_VERSION, ReadError, RegularFileStamp,
    is_normalized_absolute_path,
};

/// `O_NOFOLLOW` refuses a symlinked final component; `O_NONBLOCK` makes a
/// misidentified special file (FIFO, device) fail instead of blocking.
const SECURE_OPEN_FLAGS: i32 = nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK;

fn main() -> ExitCode {
    let mut args = env::args_os();
    let _program = args.next();
    let (binding_file, workspace) = match (
        args.next(),
        args.next(),
        args.next(),
        args.next(),
        args.next(),
    ) {
        (
            Some(binding_flag),
            Some(binding_path),
            Some(workspace_flag),
            Some(workspace_path),
            None,
        ) if binding_flag == OsStr::new("--binding-file")
            && workspace_flag == OsStr::new("--workspace") =>
        {
            (PathBuf::from(binding_path), PathBuf::from(workspace_path))
        }
        _ => {
            eprintln!("usage: rustx-execution-client --binding-file PATH --workspace PATH");
            return ExitCode::from(2);
        }
    };
    match run(&binding_file, &workspace) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("rustx-execution-client: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(binding_file: &Path, workspace: &Path) -> Result<(), String> {
    // The credential endpoint and the socket endpoint must both live under
    // owner-only directories, or either could have been planted.
    require_private_parent(binding_file)?;
    let mut file = open_private_file(binding_file)?;
    let bootstrap: Bootstrap = read_record(&mut file).map_err(|_| invalid())?;
    // The file is exactly one record; trailing bytes are a corruption.
    match file.read(&mut [0u8; 1]) {
        Ok(0) => {}
        _ => return Err(invalid()),
    }
    require_private_parent(&bootstrap.socket_path)?;
    require_socket(&bootstrap.socket_path)?;

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

    let mut stream =
        UnixStream::connect(&bootstrap.socket_path).map_err(|_| String::from("unreachable"))?;
    // One absolute budget armed when the socket is established covers
    // peer proof, the Hello write, the reply read, and the admission
    // verification — it never re-arms, so a byte-at-a-time peer cannot
    // stretch it.
    let deadline = std::time::Instant::now() + LOCAL_HANDSHAKE_TIMEOUT;
    // The credential is sent only to a peer owned by this same euid; a
    // socket planted by another user never sees it.
    require_peer_owner(&stream)?;

    let hello = Hello {
        version: PROTOCOL_VERSION,
        session: bootstrap.session,
        environment: bootstrap.environment,
        instance: bootstrap.instance,
        generation: bootstrap.generation,
        workspace: bootstrap.workspace,
        // Independently known, fixed, actually implemented capabilities —
        // never read from the file.
        capabilities: BTreeSet::from([Capability::Connection, Capability::ReadRange]),
        credential: bootstrap.credential,
    };
    let mut handshake = Handshake::new(&mut stream, deadline);
    write_record(&mut handshake, &hello).map_err(|error| handshake_error(&error))?;
    let reply: HelloReply = read_record(&mut handshake).map_err(|error| handshake_error(&error))?;
    let admission = reply.map_err(|reject| format!("rejected: {reject}"))?;
    verify_admission(&hello, &admission)?;
    handshake
        .finish()
        .map_err(|error| handshake_error(&error))?;

    serve(&mut stream, &root)
}

/// The peer's euid must equal this process's euid — the socket's other end
/// is the Server's own accept, not a planted endpoint.
#[cfg(target_os = "linux")]
fn require_peer_owner(stream: &UnixStream) -> Result<(), String> {
    let credentials =
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
            .map_err(|_| String::from("untrusted peer"))?;
    if credentials.uid() == nix::unistd::geteuid().as_raw() {
        Ok(())
    } else {
        Err(String::from("untrusted peer"))
    }
}

/// The peer's euid must equal this process's euid — the socket's other end
/// is the Server's own accept, not a planted endpoint.
#[cfg(not(target_os = "linux"))]
fn require_peer_owner(stream: &UnixStream) -> Result<(), String> {
    let (uid, _gid) =
        nix::unistd::getpeereid(stream).map_err(|_| String::from("untrusted peer"))?;
    if uid == nix::unistd::geteuid() {
        Ok(())
    } else {
        Err(String::from("untrusted peer"))
    }
}

/// Opens the bootstrap file refusing symlink and special-file endpoints,
/// then proves it is a regular file, singly linked, owned by this euid,
/// and exactly mode `0600`.
fn open_private_file(path: &Path) -> Result<fs::File, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(SECURE_OPEN_FLAGS)
        .open(path)
        .map_err(|_| invalid())?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.file_type().is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(String::from("untrusted binding file"));
    }
    Ok(file)
}

/// Requires the direct parent of `path` to be an existing non-symlink
/// owner-only (`0700`) directory owned by this euid.
fn require_private_parent(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(String::from("untrusted location"));
    };
    let metadata = fs::symlink_metadata(parent).map_err(|_| String::from("untrusted location"))?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(String::from("untrusted location"));
    }
    Ok(())
}

/// Requires `path` to exist as a real socket, not a symlink to one.
fn require_socket(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| String::from("unreachable"))?;
    if !metadata.file_type().is_socket() {
        return Err(String::from("unreachable"));
    }
    Ok(())
}

/// The admission must echo this binding's own reservation fields — its
/// version included; a server answering with different identity is not our
/// binding.
fn verify_admission(hello: &Hello, admission: &Admission) -> Result<(), String> {
    if admission.version != hello.version
        || admission.session != hello.session
        || admission.environment != hello.environment
        || admission.instance != hello.instance
        || admission.generation != hello.generation
        || admission.workspace.as_os_str() != hello.workspace.as_os_str()
    {
        return Err(String::from("admission mismatch"));
    }
    Ok(())
}

/// Serves the Server's typed requests until a clean EOF before a record —
/// a closed idle connection ends the process successfully. A truncated or
/// malformed record is an error, never a silent end.
fn serve(stream: &mut UnixStream, root: &PackageDirectory) -> Result<(), String> {
    loop {
        let Some(request) =
            read_optional_record::<ClientRequest>(stream).map_err(|_| String::from("request"))?
        else {
            return Ok(());
        };
        let reply = match request {
            ClientRequest::Ping => ClientReply::Pong,
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
        };
        write_record(stream, &reply).map_err(|_| String::from("reply"))?;
    }
}

/// Performs the one physical operation: `relative_path` must be normal
/// components only, `max_bytes` within `1..=`[`MAX_RAW_CHUNK_BYTES`], and
/// the bytes verified under the file's revision. Errors cross the wire as
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
        || relative_path.is_absolute()
        || !relative_path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
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
                metadata: RegularFileStamp {
                    size: DecimalU64::new(chunk.metadata.size),
                    revision: chunk.metadata.revision,
                    identity: chunk.metadata.identity,
                    modified_at_ms: chunk.metadata.modified_at_ms,
                },
            }
        }
        Err(error) => ClientReply::Error {
            kind: match error.kind() {
                std::io::ErrorKind::InvalidInput => ReadError::Invalid,
                std::io::ErrorKind::PermissionDenied => ReadError::Denied,
                std::io::ErrorKind::NotFound => ReadError::NotFound,
                std::io::ErrorKind::AlreadyExists => ReadError::Conflict,
                _ => ReadError::Unavailable,
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
