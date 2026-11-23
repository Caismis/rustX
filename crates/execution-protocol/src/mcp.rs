//! Execution transport for an admitted Client-owned stdio MCP session.
//! The application peer's protocol is distinct from the `RustX` binding.

use std::{collections::BTreeMap, fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DecimalU64, process::ProcessOperationKey};

/// Maximum simultaneous stdio sessions on a binding.
pub const MAX_SESSIONS: usize = 8;
/// Maximum negotiated or inbound MCP record before splitting it into pages.
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
/// A complete outbound record fits in one Execution frame after base64.
pub const MAX_SEND_BYTES: usize = 512 * 1024;
/// Pending outbound records per session.
pub const MAX_PENDING_SENDS: usize = 4;

/// The two authored source roots provisioned by the Client's host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PythonSourceScope {
    /// The deployed Client's user resource collection.
    User,
    /// The authenticated Session Workspace's package collection.
    Workspace,
}

/// Observe a bounded catalog or capture one package in the Client's view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum PythonSourceRequest {
    /// Only logical names are returned. Package files are never listed here.
    List { scope: PythonSourceScope },
    /// Reserve one capture identity before its filesystem worker starts.
    Capture {
        key: ProcessOperationKey,
        scope: PythonSourceScope,
        name: String,
    },
    /// Observe the original capture and its authenticated frozen identifier.
    State { key: ProcessOperationKey },
    /// Attach to or start preparation of one retained source identity.
    Prepare { key: ProcessOperationKey },
    /// Relinquish a source that no Server candidate or lease still owns.
    Release { key: ProcessOperationKey },
}

/// Small metadata for constructing a Server-side resource candidate.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum PythonSourceReply {
    /// Logical package names, sorted, with at most 128 entries per scope.
    Names(Vec<String>),
    /// A physically observed source root is structurally invalid.
    CollectionInvalid,
    /// One acknowledged snapshot worker is still running.
    Capturing,
    /// The stable Client Store still owns the physical preparation.
    Preparing,
    /// The Store verified the installed environment for this capture.
    Prepared,
    /// The digest identifies captured package bytes. The optional TOML is
    /// authorization metadata only and is validated by the Server.
    Captured {
        digest: String,
        permissions_base64: Option<String>,
    },
    /// Capture was released; the published cache is a separate owner.
    Released,
    /// A bounded, typed refusal without paths or authored text.
    Refused(crate::ReadError),
}

impl fmt::Debug for PythonSourceReply {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Names(names) => formatter.debug_tuple("Names").field(names).finish(),
            Self::CollectionInvalid => formatter.write_str("CollectionInvalid"),
            Self::Capturing => formatter.write_str("Capturing"),
            Self::Preparing => formatter.write_str("Preparing"),
            Self::Prepared => formatter.write_str("Prepared"),
            Self::Captured {
                digest,
                permissions_base64,
            } => formatter
                .debug_struct("Captured")
                .field("digest", digest)
                .field("has_permissions", &permissions_base64.is_some())
                .finish(),
            Self::Released => formatter.write_str("Released"),
            Self::Refused(error) => formatter.debug_tuple("Refused").field(error).finish(),
        }
    }
}

/// Immutable Client-side launch parameters. Cwd is relative to the bound root.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Launch {
    /// Executable name or a Workspace-relative or absolute Client path.
    pub program: PathBuf,
    /// Original arguments.
    pub args: Vec<String>,
    /// Workspace-relative cwd. Empty denotes the bound root.
    pub cwd: PathBuf,
    /// Explicit stdio process environment.
    pub environment: BTreeMap<String, String>,
    /// The ordered protocol versions authorized for this source.
    pub versions: Vec<String>,
    /// The authorized legacy handshake version for an external source.
    /// A managed package uses discovery only.
    pub legacy_version: Option<String>,
    /// Optional exact Client capture. A managed launch supplies no caller
    /// program, argv, cwd or environment overrides.
    pub python_source: Option<Box<ProcessOperationKey>>,
}

impl fmt::Debug for Launch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Launch")
            .field("program_configured", &!self.program.as_os_str().is_empty())
            .field("args", &self.args.len())
            .field("cwd", &self.cwd)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("versions", &self.versions)
            .field("legacy_version", &self.legacy_version)
            .field("python_source", &self.python_source)
            .finish()
    }
}

/// The authenticated Server's view of a single physical MCP session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    /// Register the identity and enqueue the first and only handshake.
    Open {
        /// Full binding identity plus a fresh operation ID.
        key: ProcessOperationKey,
        /// Frozen Client-side inputs.
        launch: Launch,
    },
    /// Get the state and, if ready, negotiation evidence.
    State {
        /// The originally registered operation.
        key: ProcessOperationKey,
    },
    /// Queue exactly one complete post-handshake MCP message.
    Send {
        /// The originally registered operation.
        key: ProcessOperationKey,
        /// The serialized message in canonical base64.
        data_base64: String,
    },
    /// Observe the next page of the next Client-owned MCP record.
    Receive {
        /// The originally registered operation.
        key: ProcessOperationKey,
        /// The expected message number, starting at one.
        sequence: DecimalU64,
        /// Offset in that message. Zero for the first page.
        offset: DecimalU64,
    },
    /// Stop a session and take its settlement evidence once available.
    Close {
        /// The originally registered operation.
        key: ProcessOperationKey,
    },
}

impl fmt::Debug for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Open { .. } => "Open",
            Self::State { .. } => "State",
            Self::Send { .. } => "Send",
            Self::Receive { .. } => "Receive",
            Self::Close { .. } => "Close",
        };
        formatter
            .debug_struct(kind)
            .field("key", self.key())
            .finish_non_exhaustive()
    }
}

impl Request {
    /// The exact identity carried by every request.
    #[must_use]
    pub const fn key(&self) -> &ProcessOperationKey {
        match self {
            Self::Open { key, .. }
            | Self::State { key }
            | Self::Send { key, .. }
            | Self::Receive { key, .. }
            | Self::Close { key } => key,
        }
    }

    /// Re-expresses the request using its parent's physical operation identity.
    #[must_use]
    pub fn with_key(mut self, physical_key: ProcessOperationKey) -> Self {
        match &mut self {
            Self::Open { key, .. }
            | Self::State { key }
            | Self::Send { key, .. }
            | Self::Receive { key, .. }
            | Self::Close { key } => *key = physical_key,
        }
        self
    }
}

/// MCP startup refusal or transport failure, without peer-controlled text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Failure {
    /// Invalid or conflicting launch or traffic.
    Invalid,
    /// Another binding owns the operation.
    Foreign,
    /// No slot is available.
    Capacity,
    /// The session has closed or does not exist.
    Unavailable,
    /// The peer negotiated no compatible revision.
    UnsupportedRevision,
    /// The handshake failed or expired.
    Handshake,
    /// A malformed or oversized MCP record closed the stream.
    InvalidRecord,
    /// A stream operation did not complete.
    Transport,
    /// The physical unit could not be proved terminal.
    SettlementUnconfirmed,
}

/// One typed reply that echoes the whole operation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    /// Binding and operation being observed.
    pub key: ProcessOperationKey,
    /// The outcome reported by its owner.
    pub reply: Reply,
}

/// A correlated reply from the physical session owner.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Reply {
    /// An operation was registered. Query State for the outcome.
    Opening,
    /// The exact negotiated SDK peer information and next unused request ID.
    Ready {
        /// JSON-encoded `ClientTransportAdmission` in canonical base64.
        admission_base64: String,
    },
    /// Nothing is waiting on this bounded lane.
    Pending,
    /// A complete outbound record entered the private queue.
    Queued,
    /// One page of one inbound message.
    Page {
        /// The expected message number.
        sequence: DecimalU64,
        /// The requested byte offset.
        offset: DecimalU64,
        /// At most one raw execution chunk, base64 encoded.
        data_base64: String,
        /// The final page ends this message.
        last: bool,
    },
    /// Shutdown was requested and the physical owner is still settling.
    Closing,
    /// The physical unit is proved terminal.
    Closed,
    /// Request refused or the owner retained a failure.
    Failed(Failure),
}

impl fmt::Debug for Reply {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opening => formatter.write_str("Opening"),
            Self::Ready { .. } => formatter.write_str("Ready"),
            Self::Pending => formatter.write_str("Pending"),
            Self::Queued => formatter.write_str("Queued"),
            Self::Page {
                sequence,
                offset,
                last,
                ..
            } => formatter
                .debug_struct("Page")
                .field("sequence", sequence)
                .field("offset", offset)
                .field("last", last)
                .finish_non_exhaustive(),
            Self::Closing => formatter.write_str("Closing"),
            Self::Closed => formatter.write_str("Closed"),
            Self::Failed(failure) => formatter.debug_tuple("Failed").field(failure).finish(),
        }
    }
}

impl Reply {
    /// A Close reply that proves there is no remaining physical session.
    #[must_use]
    pub const fn proves_close(&self) -> bool {
        matches!(
            self,
            Self::Closed
                | Self::Failed(
                    Failure::Unavailable
                        | Failure::Handshake
                        | Failure::UnsupportedRevision
                        | Failure::InvalidRecord
                        | Failure::Transport
                )
        )
    }
}
