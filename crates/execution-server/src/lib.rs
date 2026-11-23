//! Server-side binding arbitration for the rustX execution protocol.
//!
//! A `Binding` is the single arbitration point where the Server decides
//! which Client connection may receive environment dispatch. It is a
//! library owned by the Agent Server (package `rustx`). It owns paired
//! Client admission, bounded typed dispatch, and control-lane receipts.
//! The Execution Client owns physical subprocesses and file operations.

use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::net::TcpListener;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use constant_time_eq::constant_time_eq_32;
use rustx_execution_protocol::process::ServerIncarnation;
#[cfg(test)]
use rustx_execution_protocol::transport::{read_record, write_record};
use rustx_execution_protocol::watch::{
    MAX_WATCH_HINT_BYTES, MAX_WATCH_HINT_PATHS, WatchChange, WatchId, WatchReply, WatchRequest,
};
use rustx_execution_protocol::{
    Admission, BindingCredential, BootstrapEndpoint, CancelDisposition, Capability,
    ClientInstanceId, ClientReply, ClientRequest, ConnectionId, ControlHello, ControlHelloReply,
    ControlPoll, ControlReply, ControlRequest, ControlTurn, DataAdmission, DataHelloReply,
    DecimalU64, EditRefusal, EntryStamp, EnvironmentId, FileStamp, Generation, Hello, LineEnd,
    MAX_DIRECTORY_NAMES, MAX_DIRECTORY_PAGE_ENTRIES, MAX_DOCUMENT_LOGICAL_BYTES,
    MAX_DOCUMENT_SOURCE_BYTES, MAX_EDIT_FILE_BYTES, MAX_EDIT_REPLACEMENTS, MAX_EDIT_REQUEST_BYTES,
    MAX_RAW_CHUNK_BYTES, MAX_SEARCH_CONTEXT_LINES, MAX_SEARCH_PATTERN_BYTES, MAX_TEXT_BYTES,
    MAX_TEXT_LINES, MAX_WRITE_BYTES, PROTOCOL_VERSION, PageEntry, PairingNonce, PeerHello,
    ProcessCancelDisposition, ReadError, Reject, SearchSpec, SessionId, TextReplacement,
    TreeMutation, WriteExpectation, is_normalized_absolute_path,
    transport::{
        CONTROL_COMMAND_TIMEOUT, CONTROL_FRAME_TIMEOUT, EncodedRecord, MAX_CONTROL_COMMANDS,
        PAGE_REQUEST_TIMEOUT, encode_record,
    },
};
use rustx_execution_transport::{ChannelShutdown, RecordChannel};
use sha2::{Digest, Sha256};

pub mod delegation;
mod process_reply;

/// The deadline for a Client to present its Hello on an accepted local
/// connection — one shared budget owned by
/// [`rustx_execution_protocol::transport`].
pub use rustx_execution_protocol::transport::LOCAL_HANDSHAKE_TIMEOUT;

mod binding;
mod bootstrap;
mod recovery;
pub use recovery::RecoveryFence;
mod connection;
mod control;
#[cfg(test)]
mod tests;

pub use binding::{
    Binding, BindingPhase, BindingStatus, ChannelAccept, FencedHandles, Reservation,
    server_incarnation,
};
pub use bootstrap::PublishedBundle;
pub use connection::{
    CommitCancelled, ContinuationPage, DirectoryListing, DocumentPage, LinesPage, LocalConnection,
    PhysicalError, PreparedMutation, ProcessOutcomeUnknown, RangeRead, RefusedEdit, SearchResult,
    WatchOutcomeUnknown, WriteOutcomeUnknown,
};
pub use control::ControlHandle;

use binding::ConnectionOwner;
use bootstrap::{publish_bootstrap, require_private_child};
use control::{ControlShared, ControlWorker};

fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn invalid_input_static(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}
