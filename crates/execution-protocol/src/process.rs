//! Managed-process operations between the Agent Server and the physical
//! Execution Client.
//!
//! One process operation is created only by an explicit [`ProcessRequest::Register`]
//! carrying its complete immutable [`ProcessSpec`]; only an explicit
//! [`ProcessRequest::Start`] by the registered owner may cross the physical
//! START boundary. Identity is exactly [`ProcessOperationKey`] — Server
//! incarnation, Session, Client instance, binding generation, and the
//! Server-assigned [`OperationId`]. RPC and connection identifiers are
//! never part of it, and nothing here launches a job implicitly.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::{ClientInstanceId, DecimalU64, Generation, SessionId};

/// At most this many live process operations per Session.
pub const MAX_OPERATIONS_PER_SESSION: usize = 4;

/// Total active and retired process identities in one binding generation.
/// Admission reserves the eventual tombstone. The Client never evicts an
/// identity while its generation is active.
pub const MAX_PROCESS_IDENTITIES_PER_GENERATION: usize = 4096;

/// Maximum argv element count in a `ProcessProgram::Argv`.
pub const MAX_PROCESS_ARGS: usize = 256;

/// Maximum joined argv bytes.
pub const MAX_PROCESS_ARGV_BYTES: usize = 64 * 1024;

/// Maximum environment override entries.
pub const MAX_PROCESS_ENV_ENTRIES: usize = 128;

/// Maximum bytes of one environment override value.
pub const MAX_PROCESS_ENV_VALUE_BYTES: usize = 8 * 1024;

/// Maximum bytes of one environment override name.
pub const MAX_PROCESS_ENV_NAME_BYTES: usize = 256;

/// Maximum retained output per stream — the capture cap.
pub const MAX_PROCESS_OUTPUT_BYTES: usize = 256 * 1024;

/// Maximum bytes in each Agent Bash preview.
pub const BASH_PREVIEW_BYTES: usize = 16 * 1024;

/// Maximum bytes retained in a Bash continuation.
pub const MAX_BASH_CAPTURE_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum live and published Bash output files for one Client binding.
pub const MAX_BASH_CAPTURES: usize = 32;

/// Maximum bytes of one raw output/stdin data message.
pub const MAX_PROCESS_DATA_BYTES: usize = 64 * 1024;

/// Maximum total bytes of one final process result.
pub const MAX_PROCESS_FINAL_BYTES: usize = 1024 * 1024;

/// Maximum `timeout_ms` one spec may carry — the product command bound
/// of one hour, never unbounded.
pub const MAX_PROCESS_TIMEOUT_MS: u64 = 3_600_000;

/// Maximum PTY rows or columns — the product terminal bound.
pub const MAX_PTY_ROWS_COLS: u16 = 1000;

/// One Server incarnation: allocated once **process-wide** in
/// `rustx-execution-server` — never per runtime reload — and included
/// in bootstrap/admission
/// identity so operations of a dead incarnation can never be confused
/// with a later one. UUID-strength randomness; 16 bytes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ServerIncarnation(Uuid);

impl ServerIncarnation {
    /// A fresh incarnation — the **process-wide** allocation lives in
    /// `rustx-execution-server`; this crate only carries the opaque DTO.
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    /// An incarnation from explicit bytes (Client-side witness records).
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// The raw 128-bit value.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0.as_u128()
    }
}

/// One Server-assigned process operation identifier: UUID-strength
/// randomness allocated before the first dispatch, never reused within a
/// generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OperationId(Uuid);

impl OperationId {
    /// A fresh Server-assigned identifier.
    #[allow(clippy::new_without_default)] // random generation is not a default value
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// An identifier from explicit bytes (Client-side records).
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// The raw 128-bit value.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0.as_u128()
    }
}

/// The complete, hard-equality identity of one process operation.
/// Excludes every RPC or connection identifier by construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessOperationKey {
    /// The Server incarnation that assigned the operation.
    pub incarnation: ServerIncarnation,
    /// The owning Session.
    pub session: SessionId,
    /// The Client instance physically owning the process.
    pub instance: ClientInstanceId,
    /// The binding generation the operation was registered under.
    pub generation: Generation,
    /// The Server-assigned operation identifier.
    pub operation: OperationId,
}

/// What the process executes. Shell commands require Agent Bash registration.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProcessProgram {
    /// An argv vector executed directly — no shell interpretation.
    Argv(Vec<String>),
    /// A command interpreted by the supervised, non-interactive Bash shell.
    Shell {
        /// The command interpreted by Bash.
        command: String,
        /// The Server-owned Conversation authorized to read the capture.
        conversation: String,
        /// Reserve readable output at registration, for background execution
        /// and foreground invocations eligible for promotion.
        live_output: bool,
    },
}

impl fmt::Debug for ProcessProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessProgram")
            .field(
                "kind",
                &match self {
                    Self::Argv(_) => "argv",
                    Self::Shell { .. } => "shell",
                },
            )
            .field("len", &self.encoded_len())
            .finish_non_exhaustive()
    }
}

impl ProcessProgram {
    fn encoded_len(&self) -> usize {
        match self {
            Self::Argv(argv) => argv.iter().map(String::len).sum::<usize>() + argv.len(),
            Self::Shell { command, .. } => command.len(),
        }
    }
}

/// Terminal geometry and mode for a PTY allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PtySpec {
    /// Terminal rows.
    pub rows: u16,
    /// Terminal columns.
    pub cols: u16,
}

/// How stdin reaches the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessStdin {
    /// No stdin channel; the child reads EOF.
    Closed,
    /// A streamed stdin channel accepting ordered `WriteStdin` deltas.
    Streamed,
}

/// The immutable registration parameters of one process operation.
///
/// `cwd` is a normalized workspace-relative directory; the empty path is
/// the workspace root — a distinct valid value, never "unset". `env`
/// holds only authorized overrides (`None` removes a baseline key),
/// sorted by construction.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessSpec {
    /// Normalized workspace-relative working directory; empty = root.
    pub cwd: PathBuf,
    /// The program to execute.
    pub program: ProcessProgram,
    /// Authorized environment overrides, sorted; `None` unsets.
    pub env: BTreeMap<String, Option<String>>,
    /// Wall-clock timeout in milliseconds. Zero means no physical deadline for Agent Bash.
    pub timeout_ms: u64,
    /// PTY allocation, when the operation is a terminal.
    pub pty: Option<PtySpec>,
    /// Stdin delivery mode.
    pub stdin: ProcessStdin,
    /// Whether live output pages are retained for `OutputPage` queries.
    pub capture: bool,
    /// Per-stream retained-output cap — at most [`MAX_PROCESS_OUTPUT_BYTES`].
    pub output_cap: u32,
}

impl fmt::Debug for ProcessSpec {
    /// Debug output never leaks command arguments or environment contents.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessSpec")
            .field("cwd", &self.cwd)
            .field("program", &self.program)
            .field("env_entries", &self.env.len())
            .field("timeout_ms", &self.timeout_ms)
            .field("pty", &self.pty)
            .field("stdin", &self.stdin)
            .field("capture", &self.capture)
            .field("output_cap", &self.output_cap)
            .field("fingerprint", &self.fingerprint())
            .finish()
    }
}

/// A refusal to register one [`ProcessSpec`] outside the bounded schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpecRefusal {
    /// `cwd` is not normalized workspace-relative (absolute, `..`,
    /// prefix, or non-normal components).
    #[error("cwd is not a normalized workspace-relative directory")]
    InvalidCwd,
    /// More than [`MAX_PROCESS_ARGS`] argv elements.
    #[error("too many argv elements")]
    TooManyArgs,
    /// Joined argv exceeds [`MAX_PROCESS_ARGV_BYTES`].
    #[error("argv too large")]
    ArgsTooLarge,
    /// More than [`MAX_PROCESS_ENV_ENTRIES`] environment overrides.
    #[error("too many environment overrides")]
    TooManyEnvEntries,
    /// An environment name exceeds [`MAX_PROCESS_ENV_NAME_BYTES`] or a
    /// value exceeds [`MAX_PROCESS_ENV_VALUE_BYTES`].
    #[error("environment override too large")]
    EnvEntryTooLarge,
    /// `output_cap` exceeds [`MAX_PROCESS_OUTPUT_BYTES`].
    #[error("output cap exceeds bound")]
    OutputCapTooLarge,
    /// The Conversation identity is missing or malformed.
    #[error("invalid Agent Bash Conversation identity")]
    InvalidConversation,
    /// The Client could not reserve the live capture.
    #[error("live Bash output storage unavailable")]
    BashCaptureUnavailable,
    /// The registration kind does not authorize the requested program.
    #[error("program is not authorized for this registration kind")]
    ProgramNotAuthorized,
    /// The timeout exceeds [`MAX_PROCESS_TIMEOUT_MS`], or argv has no deadline.
    #[error("timeout out of bounds")]
    InvalidTimeout,
    /// PTY geometry is zero or exceeds [`MAX_PTY_ROWS_COLS`].
    #[error("pty geometry out of bounds")]
    InvalidPty,
    /// An argv element (or shell line) is empty or contains NUL.
    #[error("argv element empty or contains NUL")]
    InvalidArgv,
    /// An environment name is empty, contains `=` or NUL, or a value
    /// contains NUL.
    #[error("environment name or value invalid")]
    InvalidEnvName,
    /// The spec is valid but names a mode this Client does not physically
    /// implement — an honest refusal, never a fabricated outcome.
    #[error("mode unsupported by this client")]
    UnsupportedMode,
    /// The Session already holds [`MAX_OPERATIONS_PER_SESSION`] records
    /// (unreleased output included) — a new registration refuses.
    #[error("session operation capacity exhausted")]
    CapacityExhausted,
    /// The generation's finite tombstone space is saturated — a new
    /// registration refuses rather than evicting live evidence.
    #[error("generation tombstone capacity saturated")]
    TombstonesSaturated,
}

/// The canonical cryptographic fingerprint of one complete normalized
/// [`ProcessSpec`] — the only retained parameter record needed for
/// equality: no unbounded saved parameter data, no prefix acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpecFingerprint([u8; 32]);

impl SpecFingerprint {
    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl ProcessSpec {
    /// The immutable normalized-identity fingerprint: one tagged
    /// length-prefixed canonical encoding of every parameter field.
    #[must_use]
    pub fn fingerprint(&self) -> SpecFingerprint {
        let mut hash = Sha256::new();
        Self::field(&mut hash, 1, self.cwd.as_os_str().as_encoded_bytes());
        match &self.program {
            ProcessProgram::Argv(argv) => {
                Self::field(&mut hash, 2, b"argv");
                Self::field(
                    &mut hash,
                    3,
                    &u32::try_from(argv.len()).unwrap_or(u32::MAX).to_le_bytes(),
                );
                for (i, arg) in argv.iter().enumerate() {
                    Self::field(
                        &mut hash,
                        4,
                        &u32::try_from(i).unwrap_or(u32::MAX).to_le_bytes(),
                    );
                    Self::field(&mut hash, 5, arg.as_bytes());
                }
            }
            ProcessProgram::Shell {
                command,
                conversation,
                live_output,
            } => {
                Self::field(&mut hash, 2, b"shell");
                Self::field(&mut hash, 5, command.as_bytes());
                Self::field(&mut hash, 16, conversation.as_bytes());
                Self::field(&mut hash, 17, &[u8::from(*live_output)]);
            }
        }
        Self::field(
            &mut hash,
            6,
            &u32::try_from(self.env.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        for (name, value) in &self.env {
            Self::field(&mut hash, 7, name.as_bytes());
            match value {
                Some(value) => Self::field(&mut hash, 8, value.as_bytes()),
                None => Self::field(&mut hash, 9, b""),
            }
        }
        Self::field(&mut hash, 10, &self.timeout_ms.to_le_bytes());
        match self.pty {
            Some(pty) => {
                Self::field(&mut hash, 11, &[1]);
                Self::field(&mut hash, 12, &pty.rows.to_le_bytes());
                Self::field(&mut hash, 12, &pty.cols.to_le_bytes());
            }
            None => Self::field(&mut hash, 11, &[0]),
        }
        Self::field(
            &mut hash,
            13,
            &[u8::from(self.stdin == ProcessStdin::Streamed)],
        );
        Self::field(&mut hash, 14, &[u8::from(self.capture)]);
        Self::field(&mut hash, 15, &self.output_cap.to_le_bytes());
        SpecFingerprint(hash.finalize().into())
    }

    fn field(hash: &mut Sha256, tag: u8, bytes: &[u8]) {
        hash.update([tag]);
        hash.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
        hash.update(bytes);
    }

    /// The bounded-schema check run before any allocation or dispatch.
    ///
    /// # Errors
    /// Returns the typed refusal; a refused spec is never registered.
    pub fn validate(&self) -> Result<(), SpecRefusal> {
        let normalized = self.cwd.as_os_str().is_empty()
            || self
                .cwd
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
        if self.cwd.is_absolute() || !normalized {
            return Err(SpecRefusal::InvalidCwd);
        }
        if let ProcessProgram::Argv(argv) = &self.program {
            if argv.len() > MAX_PROCESS_ARGS {
                return Err(SpecRefusal::TooManyArgs);
            }
            if argv.iter().map(String::len).sum::<usize>() > MAX_PROCESS_ARGV_BYTES {
                return Err(SpecRefusal::ArgsTooLarge);
            }
        }
        if self.env.len() > MAX_PROCESS_ENV_ENTRIES {
            return Err(SpecRefusal::TooManyEnvEntries);
        }
        for (name, value) in &self.env {
            if name.len() > MAX_PROCESS_ENV_NAME_BYTES
                || value.as_ref().map_or(0, String::len) > MAX_PROCESS_ENV_VALUE_BYTES
            {
                return Err(SpecRefusal::EnvEntryTooLarge);
            }
        }
        if self.output_cap > u32::try_from(MAX_PROCESS_OUTPUT_BYTES).unwrap_or(u32::MAX) {
            return Err(SpecRefusal::OutputCapTooLarge);
        }
        if self.timeout_ms > MAX_PROCESS_TIMEOUT_MS
            || (self.timeout_ms == 0 && matches!(self.program, ProcessProgram::Argv(_)))
        {
            return Err(SpecRefusal::InvalidTimeout);
        }
        if let Some(pty) = self.pty
            && (pty.rows == 0
                || pty.cols == 0
                || pty.rows > MAX_PTY_ROWS_COLS
                || pty.cols > MAX_PTY_ROWS_COLS)
        {
            return Err(SpecRefusal::InvalidPty);
        }
        match &self.program {
            ProcessProgram::Argv(argv) => {
                if argv.first().is_none_or(String::is_empty)
                    || argv.iter().any(|arg| arg.contains('\0'))
                {
                    return Err(SpecRefusal::InvalidArgv);
                }
            }
            ProcessProgram::Shell {
                command,
                conversation,
                ..
            } => {
                if conversation.is_empty()
                    || conversation.len() > 64
                    || !conversation
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return Err(SpecRefusal::InvalidConversation);
                }
                if self.pty.is_some() || self.stdin != ProcessStdin::Closed {
                    return Err(SpecRefusal::UnsupportedMode);
                }
                if command.len() > MAX_PROCESS_ARGV_BYTES {
                    return Err(SpecRefusal::ArgsTooLarge);
                }
                if command.is_empty() || command.contains('\0') {
                    return Err(SpecRefusal::InvalidArgv);
                }
            }
        }
        for (name, value) in &self.env {
            if name.is_empty()
                || name.contains('=')
                || name.contains('\0')
                || value.as_ref().is_some_and(|v| v.contains('\0'))
            {
                return Err(SpecRefusal::InvalidEnvName);
            }
        }
        Ok(())
    }
}

/// The publishable lifecycle state of one operation. `Running` may be
/// published only after physical START (or a stronger actual witness);
/// `Exited` comes only from an actual process event — a terminal state is
/// never claimed early.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationState {
    /// Registered, never started.
    Registered,
    /// START admitted at the Server; physical spawn not yet witnessed.
    Starting,
    /// Physical START observed — the actual spawn witness.
    Running,
    /// The process exited; drain/reap not yet settled.
    Exited {
        /// The observed exit code, when the platform reported one.
        exit_code: Option<i32>,
    },
    /// The operation driver finalized its outcome and journal. Physical
    /// settlement is independently reported by the snapshot.
    Settled(SettledSnapshot),
}

/// The honest terminal snapshot carried by `Settled` states and replies —
/// complete enough that the Server can produce an exact result even when
/// it never observed an intermediate `Exited` publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettledSnapshot {
    /// The terminal disposition.
    pub disposition: SettledDisposition,
    /// The process never started, or its containment and reap were proven.
    /// This remains available after output is released. An unconfirmed
    /// snapshot carries `Unknown` and no exit code.
    pub physical_settlement_proven: bool,
    /// The observed exit code, absent for an `Unknown` outcome.
    pub exit_code: Option<i32>,
    /// Total bytes the process emitted on stdout (cap-independent).
    pub stdout_total: DecimalU64,
    /// Total bytes the process emitted on stderr (cap-independent).
    pub stderr_total: DecimalU64,
    /// `true` when stdout retention reached the cap — the retained page
    /// is the earliest `output_cap` bytes; later bytes were dropped.
    pub stdout_truncated: bool,
    /// `true` when stderr retention reached the cap, same retention rule.
    pub stderr_truncated: bool,
    /// The final ordered-output sequence — the `ReleaseOutput` boundary.
    pub final_sequence: DecimalU64,
}

/// The terminal disposition of a settled operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettledDisposition {
    /// Exited with an observed status — `exit_code` is authoritative.
    Known,
    /// The business outcome could not be witnessed honestly. Containment
    /// may still be proven independently.
    Unknown,
    /// The spec's timeout elapsed; distinct from an external cancel.
    TimedOut,
    /// Cancelled — physical containment was awaited.
    Cancelled,
    /// Failed to start or to settle.
    Failed,
}

/// Which output stream one page reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessStream {
    /// Combined PTY output.
    Pty,
    /// Pipe-mode stdout.
    Stdout,
    /// Pipe-mode stderr.
    Stderr,
}

/// A bounded terminal projection of the physical Bash capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashCaptureResult {
    /// Separate decoded stdout preview.
    pub stdout: String,
    /// Separate decoded stderr preview.
    pub stderr: String,
    /// Interleaved decoded output preview.
    pub combined: String,
    /// True when at least one preview omitted text.
    pub truncated: bool,
    /// Size of the complete logical output when known.
    pub total: Option<DecimalU64>,
    /// State of the managed continuation.
    pub output: BashCaptureState,
    /// The Client supervisor witnessed its physical START grant.
    pub started: bool,
    /// A natural signal termination, when a numeric signal was observed.
    pub signal: Option<i32>,
}

/// Availability and completeness of the Client-owned continuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BashCaptureState {
    /// All text fits the previews.
    None,
    /// The full logical output was stored continuously.
    Complete,
    /// A readable initial part survives a capture failure.
    Partial,
    /// The capture failed and there is no readable output to advertise.
    Unavailable,
}

/// One Server→Client data-lane process request. Every request carries
/// the complete [`ProcessOperationKey`] — hard-equality gated; RPC and
/// connection ids never participate. Cancellation for a live operation
/// arrives on the control lane as `CancelProcess`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProcessRequest {
    /// Allocate an operation record under `key`. A replay presenting the
    /// identical complete key and spec fingerprint attaches the existing
    /// record — never spawns a second job; a key with a different
    /// fingerprint is `Conflict`. Registration never launches.
    Register {
        /// The complete operation identity the Server assigned.
        key: ProcessOperationKey,
        /// The complete immutable parameters.
        spec: ProcessSpec,
        /// The canonical parameter fingerprint.
        fingerprint: SpecFingerprint,
    },
    /// Register an authorized Agent Bash invocation. This internal entry
    /// point requires the Agent Bash capability. App Server command/exec
    /// sends only the argv registration above.
    RegisterAgentBash {
        /// The complete operation identity the Server assigned.
        key: ProcessOperationKey,
        /// The immutable shell launch parameters.
        spec: ProcessSpec,
        /// The canonical parameter fingerprint.
        fingerprint: SpecFingerprint,
    },
    /// Fetch the bounded preview after the Bash operation settles.
    BashCapture {
        /// The operation whose capture is requested.
        key: ProcessOperationKey,
    },
    /// Read the bounded tail of a live Agent Bash capture.
    BashOutputTail {
        /// The operation whose live output is requested.
        key: ProcessOperationKey,
        /// The authorized Conversation.
        conversation: String,
    },
    /// Delete the retained continuation of a settled Bash operation.
    DiscardBashOutput {
        /// The operation that owns the output.
        key: ProcessOperationKey,
    },
    /// Admit START for the keyed `Registered` operation. The reply is a
    /// verified state snapshot — `Running` only when physical START was
    /// actually witnessed by `try_start`; otherwise the honest pre-START
    /// or failed state. The only message that may spawn.
    Start {
        /// The operation to start.
        key: ProcessOperationKey,
    },
    /// Observe an operation's state. A miss is `Unknown{key}` — a query
    /// never allocates, never launches.
    Query {
        /// The operation to observe.
        key: ProcessOperationKey,
    },
    /// Page the operation's ordered output journal. `cursor` is the
    /// global ordered-output sequence to resume from — one interleaved
    /// order across streams, each page tagged with its stream and byte
    /// offset within that stream.
    OutputPage {
        /// The operation owning the journal.
        key: ProcessOperationKey,
        /// Resume strictly after this ordered-output sequence; `0` asks
        /// for the retained prefix from the beginning.
        cursor: DecimalU64,
        /// At most [`MAX_PROCESS_DATA_BYTES`] decoded bytes.
        max_bytes: u32,
    },
    /// Deliver one ordered stdin delta — shares the single input
    /// sequence with `CloseStdin` and `Resize`. The same sequence and input
    /// can attach while its receipt is retained. A released receipt cannot
    /// authorize a replay.
    WriteStdin {
        /// The operation owning stdin.
        key: ProcessOperationKey,
        /// The strict per-operation input sequence.
        sequence: DecimalU64,
        /// The bytes, base64; at most [`MAX_PROCESS_DATA_BYTES`] decoded.
        delta_base64: String,
        /// Close stdin after this delta — one atomic admission: when
        /// the live PTY policy makes EOF impossible, the whole batch is
        /// `Rejected` and no byte is written.
        #[serde(default)]
        close: bool,
    },
    /// Close the stdin channel at this input-sequence position.
    CloseStdin {
        /// The operation owning stdin.
        key: ProcessOperationKey,
        /// The input sequence this close follows.
        sequence: DecimalU64,
    },
    /// Resize the operation's PTY at this input-sequence position.
    Resize {
        /// The operation owning the terminal.
        key: ProcessOperationKey,
        /// The input sequence this resize occupies.
        sequence: DecimalU64,
        /// The new geometry; acknowledged in delivery order.
        size: PtySpec,
    },
    /// Poll one admitted input control and take its terminal receipt.
    /// A pending poll leaves the receipt owned by this input sequence
    /// unless the Server ends observation with `abandon_pending`.
    InputReceipt {
        /// The operation owning the input sequence.
        key: ProcessOperationKey,
        /// The admitted input sequence to observe.
        sequence: DecimalU64,
        /// Release a pending receipt without cancelling the physical input.
        abandon_pending: bool,
    },
    /// Acknowledge consumption of all journal entries through
    /// `final_sequence`: bulky retained buffers are freed; the minimal
    /// operation tombstone (identity + settled snapshot) remains.
    ReleaseOutput {
        /// The operation being released.
        key: ProcessOperationKey,
        /// The highest ordered-output sequence the Server consumed or discarded.
        final_sequence: DecimalU64,
    },
}

impl ProcessRequest {
    /// The complete operation key this request carries — the one its
    /// reply must echo exactly.
    #[must_use]
    pub fn key(&self) -> &ProcessOperationKey {
        match self {
            Self::Register { key, .. }
            | Self::RegisterAgentBash { key, .. }
            | Self::DiscardBashOutput { key }
            | Self::BashCapture { key }
            | Self::BashOutputTail { key, .. }
            | Self::Start { key }
            | Self::Query { key }
            | Self::OutputPage { key, .. }
            | Self::WriteStdin { key, .. }
            | Self::CloseStdin { key, .. }
            | Self::Resize { key, .. }
            | Self::InputReceipt { key, .. }
            | Self::ReleaseOutput { key, .. } => key,
        }
    }
}

/// The physical outcome of one admitted input control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputOutcome {
    /// Admitted and queued; the physical operation has not completed.
    Pending,
    /// The physical stdin/PTY operation was actually applied.
    Applied,
    /// The physical operation failed. An input prefix may already have applied.
    Failed,
    /// The physical layer vetoed the operation before applying it —
    /// e.g. a byte-delivered EOF under a raw-mode termios policy.
    Rejected,
    /// The sequence was never admitted or its receipt was already
    /// collected or abandoned.
    Unknown,
}

/// One Client→Server process reply. Every keyed reply echoes the exact
/// supplied key — no secondary correlation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProcessReply {
    /// The operation is registered — fresh allocation or replay attach.
    Registered {
        /// The exact supplied key.
        key: ProcessOperationKey,
    },
    /// The request collided with an existing record — same key,
    /// different fingerprint, wrong owner, or illegal state transition.
    /// Never a launch.
    Conflict {
        /// The exact supplied key.
        key: ProcessOperationKey,
    },
    /// No record matches the exact supplied key — including a stale
    /// instance/generation/incarnation.
    Unknown {
        /// The exact supplied key.
        key: ProcessOperationKey,
    },
    /// The operation's verified publishable state — the `Start` and
    /// `Query` answer alike.
    State {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// What is actually known — never early-terminal.
        state: OperationState,
    },
    /// Nothing new beyond the cursor while the operation is still live —
    /// the page boundary itself is honest, never `Unknown`.
    OutputPending {
        /// The exact supplied key.
        key: ProcessOperationKey,
    },
    /// The cursor already covers the complete settled journal — the
    /// caller learned `final_sequence` from the settled snapshot.
    OutputEnd {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// The settled journal's final ordered-output sequence.
        final_sequence: DecimalU64,
    },
    /// One ordered output page of the interleaved journal.
    Output {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// The global ordered-output sequence of this entry — the next
        /// `OutputPage` cursor and the `ReleaseOutput` boundary unit.
        sequence: DecimalU64,
        /// The stream this entry belongs to.
        stream: ProcessStream,
        /// The byte offset of `delta`'s first byte within `stream`.
        offset: DecimalU64,
        /// The bytes, base64; at most the requested bound.
        delta_base64: String,
        /// The stream's retained prefix is capped — later bytes dropped.
        capped: bool,
        /// This is the journal's final entry.
        last: bool,
    },
    /// One ordered input control was admitted at `sequence` into the
    /// bounded input queue — the physical outcome is observed through
    /// `InputReceipt`, never by blocking the data lane.
    InputAccepted {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// The admitted input sequence.
        sequence: DecimalU64,
    },
    /// The physical receipt for one admitted input control.
    InputReceipt {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// The queried input sequence.
        sequence: DecimalU64,
        /// The physical outcome at this poll. `Unknown` means the
        /// sequence has no outstanding receipt.
        status: InputOutcome,
    },
    /// Journal bytes through `final_sequence` released; the minimal
    /// tombstone remains queryable.
    OutputReleased {
        /// The exact supplied key.
        key: ProcessOperationKey,
    },
    /// The requested Bash capture has settled.
    BashCapture {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// The bounded capture, absent for a pre-START terminal.
        capture: Option<BashCaptureResult>,
    },
    /// The requested live Bash output tail.
    BashOutputTail {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// The UTF-8 text retained within the preview bound.
        text: String,
        /// The capture started before the first returned byte.
        truncated: bool,
    },
    /// The bounded schema refused a registration before allocation.
    Refused {
        /// The exact supplied key.
        key: ProcessOperationKey,
        /// The typed refusal.
        reason: SpecRefusal,
    },
}

impl ProcessReply {
    /// The exact key this reply echoes — equal to its request's key on
    /// every honest reply.
    #[must_use]
    pub fn key(&self) -> &ProcessOperationKey {
        match self {
            Self::Registered { key }
            | Self::Conflict { key }
            | Self::Unknown { key }
            | Self::State { key, .. }
            | Self::OutputPending { key }
            | Self::OutputEnd { key, .. }
            | Self::Output { key, .. }
            | Self::InputAccepted { key, .. }
            | Self::InputReceipt { key, .. }
            | Self::OutputReleased { key }
            | Self::BashCapture { key, .. }
            | Self::BashOutputTail { key, .. }
            | Self::Refused { key, .. } => key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ProcessSpec {
        ProcessSpec {
            cwd: PathBuf::from("sub/dir"),
            program: ProcessProgram::Argv(vec!["/bin/echo".into(), "hi".into()]),
            env: BTreeMap::from([("A".into(), Some("1".into())), ("B".into(), None)]),
            timeout_ms: 5_000,
            pty: None,
            stdin: ProcessStdin::Streamed,
            capture: true,
            output_cap: 1024,
        }
    }

    #[test]
    fn fingerprint_is_deterministic_and_complete() {
        assert_eq!(spec().fingerprint(), spec().fingerprint());
        let mut other = spec();
        other.env.insert("C".into(), Some("x".into()));
        assert_ne!(spec().fingerprint(), other.fingerprint());
        let mut other = spec();
        other.cwd = PathBuf::new();
        assert_ne!(spec().fingerprint(), other.fingerprint());
    }

    #[test]
    fn workspace_root_is_a_valid_distinct_cwd() {
        let mut root = spec();
        root.cwd = PathBuf::new();
        assert_eq!(root.validate(), Ok(()));
        for bad in [
            PathBuf::from("/abs"),
            PathBuf::from("../up"),
            PathBuf::from("./dot"),
        ] {
            let mut s = spec();
            s.cwd = bad;
            assert_eq!(s.validate(), Err(SpecRefusal::InvalidCwd));
        }
    }

    #[test]
    fn bounds_are_enforced() {
        let mut s = spec();
        s.output_cap = u32::try_from(MAX_PROCESS_OUTPUT_BYTES).unwrap() + 1;
        assert_eq!(s.validate(), Err(SpecRefusal::OutputCapTooLarge));
        let mut s = spec();
        s.program = ProcessProgram::Argv(vec!["x".into(); MAX_PROCESS_ARGS + 1]);
        assert_eq!(s.validate(), Err(SpecRefusal::TooManyArgs));
        let mut s = spec();
        s.env = (0..=MAX_PROCESS_ENV_ENTRIES)
            .map(|i| (format!("K{i}"), Some("v".into())))
            .collect();
        assert_eq!(s.validate(), Err(SpecRefusal::TooManyEnvEntries));
        let mut s = spec();
        s.env = BTreeMap::from([(
            "K".into(),
            Some("v".repeat(MAX_PROCESS_ENV_VALUE_BYTES + 1)),
        )]);
        assert_eq!(s.validate(), Err(SpecRefusal::EnvEntryTooLarge));
        let mut s = spec();
        s.timeout_ms = 0;
        assert_eq!(s.validate(), Err(SpecRefusal::InvalidTimeout));
        let mut s = spec();
        s.pty = Some(PtySpec { rows: 0, cols: 24 });
        assert_eq!(s.validate(), Err(SpecRefusal::InvalidPty));
        let mut s = spec();
        s.program = ProcessProgram::Argv(vec!["a\0b".into()]);
        assert_eq!(s.validate(), Err(SpecRefusal::InvalidArgv));
        let mut s = spec();
        s.env = BTreeMap::from([("A=B".into(), None)]);
        assert_eq!(s.validate(), Err(SpecRefusal::InvalidEnvName));
    }

    #[test]
    fn argv_allows_empty_arguments_but_rejects_an_empty_program_or_a_nul() {
        let mut input = spec();
        input.program =
            ProcessProgram::Argv(vec!["/usr/bin/printf".into(), "<%s>".into(), String::new()]);
        assert_eq!(input.validate(), Ok(()));
        let with_placeholder = input.fingerprint();
        input.program = ProcessProgram::Argv(vec!["/usr/bin/printf".into(), "<%s>".into()]);
        assert_ne!(input.fingerprint(), with_placeholder);
        input.program = ProcessProgram::Argv(vec![String::new(), "arg".into()]);
        assert_eq!(input.validate(), Err(SpecRefusal::InvalidArgv));
        input.program = ProcessProgram::Argv(vec!["printf".into(), "a\0b".into()]);
        assert_eq!(input.validate(), Err(SpecRefusal::InvalidArgv));
    }

    #[test]
    fn debug_never_leaks_command_or_env() {
        let text = format!("{:?}", spec());
        assert!(!text.contains("/bin/echo") && !text.contains("hi") && !text.contains("\"A\""));
    }

    #[test]
    fn incarnation_dto_roundtrips() {
        let i = ServerIncarnation::generate();
        assert_eq!(
            ServerIncarnation::from_uuid(Uuid::from_u128(i.as_u128())),
            i
        );
    }
}
