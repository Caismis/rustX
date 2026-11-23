//! Single wire definition for the rustX execution protocol (v2).
//!
//! The execution protocol is the physical environment/output channel between
//! the Agent Server (package `rustx`) and the physical Execution Client
//! (package `rustx-execution-client`). Every type here is the one serde DTO
//! both sides speak; nothing in this crate advertises implemented I/O. An
//! authenticated [`Hello`]/[`Admission`] exchange proves binding ownership
//! only — environment readiness facts (files, configuration, MCP, Python)
//! are separate, later reports.

use std::collections::BTreeSet;
use std::fmt;
use std::num::NonZeroU64;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

pub mod changes;
pub mod process;
pub mod save;
pub mod transport;
pub mod watch;

/// The only protocol revision this implementation speaks. Incompatible
/// versions are explicitly rejected; there is no implicit downgrade.
pub const PROTOCOL_VERSION: u16 = 2;

/// WebSocket path for this wire revision.
pub const EXECUTION_PATH: &str = "/execution/v2";

/// Maximum size of one encoded transport frame, envelope included (1 MiB).
/// Enforcement happens at the transport read/parse boundary before
/// allocation; raw payload size never relaxes this bound.
pub const MAX_ENCODED_FRAME_BYTES: usize = 1024 * 1024;

/// Maximum size of one raw content chunk before encoding (64 KiB). Encoding
/// expansion still counts against [`MAX_ENCODED_FRAME_BYTES`].
pub const MAX_RAW_CHUNK_BYTES: usize = 64 * 1024;

/// Maximum names one directory may hold for paging — the listing cap
/// `Directory` pages are paginated through.
pub const MAX_DIRECTORY_NAMES: usize = 4096;

/// Maximum entries one `Directory` page reply may carry.
pub const MAX_DIRECTORY_PAGE_ENTRIES: usize = 128;

/// Maximum joined bytes of one `Lines` page — the wire policy equal to the
/// physical scanner's 50KB complete-line budget.
pub const MAX_TEXT_BYTES: usize = 50 * 1024;

/// Maximum counted complete lines of one `Lines` page — the wire policy
/// equal to the physical scanner's 2000-line budget. A page that ends
/// exactly on a source trailing newline may show one extra empty element.
pub const MAX_TEXT_LINES: usize = 2_000;

/// Maximum source bytes of one `ReadDocument` target — the wire policy
/// equal to the shared document decoder's extraction-resource bound. A
/// source larger than this is refused before any decode work starts, and
/// a `DocumentPage` claiming a larger `source` is untruthful.
pub const MAX_DOCUMENT_SOURCE_BYTES: usize = 32 * 1024 * 1024;

/// Maximum logical bytes of the decoded Markdown text one `DocumentPage`
/// may describe — the wire policy equal to the shared document decoder's
/// projection bound. `logical_bytes` counts the decoded text, independent
/// of the compressed source's byte length.
pub const MAX_DOCUMENT_LOGICAL_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum bytes of one `Search` pattern or file-filter glob — the wire
/// policy, enforced before any traversal starts.
pub const MAX_SEARCH_PATTERN_BYTES: usize = 8 * 1024;

/// Maximum surrounding context lines one `Search` grep may select.
pub const MAX_SEARCH_CONTEXT_LINES: u32 = 20;

/// The grep record preview: at most this many Unicode scalars per line
/// before the `... [truncated]` marker, which `short_lines` reports.
pub const SEARCH_PREVIEW_SCALARS: usize = 500;

/// The bounded traversal's maximum directory depth below the search root.
pub const MAX_SEARCH_DEPTH: u32 = 64;

/// The bounded traversal's total retained relative-path bytes.
pub const MAX_SEARCH_PATH_BYTES: usize = 8 * 1024 * 1024;

/// The bounded traversal's total visited directory entries.
pub const MAX_SEARCH_ENTRIES: u64 = 65_536;

/// The bounded grep working buffer per file — the searcher's heap limit
/// and the regex compiled/determinization budgets share this wire policy.
pub const MAX_SEARCH_BUFFER_BYTES: usize = 8 * 1024 * 1024;

/// The byte cap on both the conditional-edit input and its produced
/// output — the kernel enforces the same constant, so a `PrepareEdit`
/// can never stage over it.
pub const MAX_EDIT_FILE_BYTES: usize = 2 * 1024 * 1024;

/// The aggregate old+new byte cap of one `PrepareEdit`'s replacements.
pub const MAX_EDIT_REQUEST_BYTES: usize = 64 * 1024;

/// The maximum number of replacements in one `PrepareEdit`.
pub const MAX_EDIT_REPLACEMENTS: usize = 128;

/// The byte cap on a conditional write's planned content — the same
/// bound a staged result may occupy.
pub const MAX_WRITE_BYTES: usize = MAX_EDIT_FILE_BYTES;

/// Live ticketed preparations per connection — writes and edits share
/// this one budget.
pub const MAX_PREPARATIONS_PER_CONNECTION: usize = 8;

/// What a ticketed write or edit prepare expects at the named leaf.
///
/// `Current` retains whatever is present — the pre-authorization
/// object an Agent prepares before its permission gate; it is never a
/// permission to publish, and the ticketed commit re-proves the leaf
/// before crossing. `Absent` is create-only; `Revision` requires the
/// exact current stamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum WriteExpectation {
    /// Retain whatever the leaf currently is — file present or absent.
    Current,
    /// Create-only: the leaf must be absent at prepare and at publish.
    Absent,
    /// The leaf must be an existing file stamped with exactly this
    /// revision.
    Revision(String),
}

/// The closed rustX-owned whitelist of structured document formats a
/// `ReadDocument` may decode.
///
/// Adding a variant here is the only way a new model-facing format can
/// appear; a parser gaining support for another format never changes the
/// contract on its own. The whitelist is rustX policy shared verbatim by
/// the Execution Client and the native Read tool — there is exactly one
/// classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    /// Portable Document Format.
    Pdf,
    /// Office Open XML word processing document.
    Docx,
    /// Office Open XML spreadsheet.
    Xlsx,
    /// Office Open XML presentation.
    Pptx,
}

impl DocumentFormat {
    /// The human-readable format label used in deterministic failure
    /// diagnostics.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Docx => "DOCX",
            Self::Xlsx => "XLSX",
            Self::Pptx => "PPTX",
        }
    }
}

/// The rustX-owned source classification of one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// An ordinary byte source decoded as UTF-8 text by the faithful text
    /// path.
    Text,
    /// A whitelisted structured document decoded through the shared
    /// parser policy.
    Document(DocumentFormat),
}

/// Classifies one already-interpreted target by its extension.
///
/// The comparison is deterministic: the extension is matched
/// case-insensitively against the closed whitelist, and only the final
/// extension component participates. Content is never sniffed to expand or
/// redirect the whitelist; a mislabeled file is decoded by exactly the
/// decoder its extension selects.
#[must_use]
pub fn classify_source(target: &std::path::Path) -> SourceKind {
    let Some(extension) = target.extension().and_then(|e| e.to_str()) else {
        return SourceKind::Text;
    };
    let normalized = extension.to_ascii_lowercase();
    let format = match normalized.as_str() {
        "pdf" => DocumentFormat::Pdf,
        "docx" => DocumentFormat::Docx,
        "xlsx" => DocumentFormat::Xlsx,
        "pptx" => DocumentFormat::Pptx,
        _ => return SourceKind::Text,
    };
    SourceKind::Document(format)
}

/// Maximum data records queued or in flight on one connection.
pub const MAX_DATA_RECORDS_PER_CONNECTION: usize = 8;

/// Maximum control records queued or in flight on one connection. Control
/// progress is independent of data-record saturation.
pub const MAX_CONTROL_RECORDS_PER_CONNECTION: usize = 4;

/// Absolute deadline for one optional Server → Client managed-output save.
/// The save budget is independent of the business-receive budget and never
/// extends the calling operation's remaining deadline.
pub const OPTIONAL_SAVE_DEADLINE: Duration = Duration::from_secs(5);

macro_rules! identity_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates the identity from an opaque string value.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The identity as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

identity_newtype!(
    SessionId,
    "The Session a binding belongs to. One Session has at most one live Execution Client."
);
identity_newtype!(
    EnvironmentId,
    "The logical execution location. Not a path, and not a machine-isolation promise."
);
identity_newtype!(
    ClientInstanceId,
    "One physical Client process lifetime. An instance identity is not a credential."
);
identity_newtype!(
    ConnectionId,
    "One transport connection. A connection is never the execution owner."
);

/// A canonical decimal `u64` on the wire.
///
/// The wire form is exactly one non-negative decimal string: `0` is valid,
/// but no sign, no leading zero, no padding, and no `u64` overflow. It is
/// used for offsets and other positions where zero is meaningful;
/// [`Generation`] is the same wire form with zero excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalU64(u64);

impl DecimalU64 {
    /// Creates the value from a `u64`.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The value as a `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The shared canonical-decimal wire rule: digits only, no leading zero
/// except the single digit `0`, no overflow.
fn parse_canonical_decimal(text: &str) -> Option<u64> {
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    text.parse().ok()
}

/// Failure to parse a canonical decimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("value must be a canonical decimal string")]
pub struct DecimalParseError;

impl FromStr for DecimalU64 {
    type Err = DecimalParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        parse_canonical_decimal(text)
            .map(Self)
            .ok_or(DecimalParseError)
    }
}

impl From<u64> for DecimalU64 {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for DecimalU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DecimalVisitor;

        impl Visitor<'_> for DecimalVisitor {
            type Value = DecimalU64;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a canonical decimal string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(DecimalVisitor)
    }
}

/// The current binding generation.
///
/// Same-instance reconnects keep the generation; an instance replacement
/// increments it. On the wire a generation is exactly one canonical positive
/// decimal string: no sign, no leading zero, no padding, no overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(NonZeroU64);

impl Generation {
    /// Creates a generation from a nonzero value.
    #[must_use]
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    /// The generation as a `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Failure to parse a canonical generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("generation must be a canonical positive decimal string")]
pub struct GenerationParseError;

impl FromStr for Generation {
    type Err = GenerationParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = parse_canonical_decimal(text).ok_or(GenerationParseError)?;
        Ok(Self(NonZeroU64::new(value).ok_or(GenerationParseError)?))
    }
}

impl TryFrom<u64> for Generation {
    type Error = GenerationParseError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(Self(NonZeroU64::new(value).ok_or(GenerationParseError)?))
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Generation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Generation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct GenerationVisitor;

        impl Visitor<'_> for GenerationVisitor {
            type Value = Generation;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a canonical positive decimal string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(GenerationVisitor)
    }
}

/// A physical environment capability a Client can prove.
///
/// The vocabulary is the plan's v1 capability set. Advertising a capability
/// claims the Client owns the physical responsibility; it never implies the
/// feature is ready or dispatchable today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Authenticated connection, identity, health round-trips, binding state.
    Connection,
    /// Workspace file I/O: read, write, conditional replace, metadata,
    /// directory paging, copy/delete, change notifications.
    Files,
    /// The one implemented byte operation: bounded revision-checked
    /// range reads under the binding workspace.
    ReadRange,
    /// The implemented query operations: entry metadata stat'ed without
    /// following the leaf, and bounded directory paging, under the
    /// binding workspace.
    DirectoryMetadata,
    /// Descriptor-relative directory creation, tree removal and tree copy.
    TreeMutations,
    /// Non-recursive native workspace watch hints.
    FileWatches,
    /// Client-owned Workspace baselines, file observations, and comparisons.
    WorkspaceChanges,
    /// The bound Client prepares Workspace Python for native Agent Bash.
    WorkspacePython,
    /// The implemented conditional write: one staged `PrepareWrite` plus a
    /// ticketed `CommitWrite`/`AbortWrite` per request, carried out by the
    /// private commit kernel under the binding workspace.
    ConditionalWrite,
    /// The implemented conditional edit: one `PrepareEdit` that reads the
    /// retained original, applies the bounded text-edit algorithm, and
    /// stages the result privately, published by the same ticketed
    /// `CommitWrite`/`AbortWrite` machinery as `ConditionalWrite`.
    ConditionalEdit,
    /// The implemented logical-text page: a bounded streaming line page
    /// under the binding workspace with whole-source UTF-8 validation.
    ReadLines,
    /// The implemented structured-document page: a whitelisted document
    /// under the binding workspace decoded by the shared parser policy
    /// into bounded logical-text pages.
    DocumentPages,
    /// The implemented bounded workspace search: one `Search` request
    /// carries the shared file-universe traversal and the selected
    /// glob/grep record projection under the binding workspace.
    BoundedSearch,
    /// Managed processes under Server-assigned `OperationId`s.
    Process,
    /// Registration for authorized native Agent Bash commands.
    AgentBash,
    /// Workspace `uv` and Managed Python fingerprint environments.
    Python,
    /// Client-hosted stdio MCP sessions.
    StdioMcp,
    /// Bounded byte-chunk content transfer and upload staging.
    ContentTransfer,
    /// Client-owned managed output: private allocation, unpublished
    /// candidate staging, seal/publish, loser cleanup.
    ManagedOutput,
}

/// The binding credential presented in a [`Hello`].
///
/// Exactly 32 bytes of OS randomness, carried on the wire as 64 lowercase
/// hex characters. The secret is never stored on the Server — only its
/// SHA-256 verifier is — and it is never echoed: `Debug` redacts, there is
/// no `Display`, and parse errors do not contain the submitted value.
#[derive(Clone)]
pub struct BindingCredential([u8; Self::LENGTH]);

impl BindingCredential {
    /// The credential length in bytes.
    pub const LENGTH: usize = 32;
    /// The credential length on the wire in hex characters.
    pub const HEX_LENGTH: usize = Self::LENGTH * 2;

    /// Wraps raw credential bytes issued by the Server.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    /// The raw credential bytes, for verifier computation only.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

/// Failure to parse a wire credential. Never carries the submitted value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("binding credential must be exactly 64 lowercase hex characters")]
pub struct CredentialParseError;

impl FromStr for BindingCredential {
    type Err = CredentialParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != Self::HEX_LENGTH {
            return Err(CredentialParseError);
        }
        let mut bytes = [0u8; Self::LENGTH];
        let (pairs, _) = text.as_bytes().as_chunks::<2>();
        for (slot, &[high, low]) in bytes.iter_mut().zip(pairs) {
            let high = hex_value(high).ok_or(CredentialParseError)?;
            let low = hex_value(low).ok_or(CredentialParseError)?;
            *slot = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

impl fmt::Debug for BindingCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BindingCredential(REDACTED)")
    }
}

impl Serialize for BindingCredential {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut hex = String::with_capacity(Self::HEX_LENGTH);
        for byte in self.0 {
            use fmt::Write;
            write!(hex, "{byte:02x}").expect("string write");
        }
        serializer.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for BindingCredential {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CredentialVisitor;

        impl Visitor<'_> for CredentialVisitor {
            type Value = BindingCredential;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("64 lowercase hex characters")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(CredentialVisitor)
    }
}

/// The pairing challenge issued to an admitted data lane.
///
/// Exactly 32 bytes of OS randomness carried on the wire as 64 lowercase
/// hex characters — the same shape and secrecy rules as
/// [`BindingCredential`]: `Debug` redacts, there is no `Display`, and the
/// Server retains only a SHA-256 verifier. It authenticates the control
/// lane to the one pending pairing attempt; it is never reusable across
/// attempts or bindings.
#[derive(Clone)]
pub struct PairingNonce([u8; Self::LENGTH]);

impl PairingNonce {
    /// The nonce length in bytes.
    pub const LENGTH: usize = 32;
    /// The nonce length on the wire in hex characters.
    pub const HEX_LENGTH: usize = Self::LENGTH * 2;

    /// Wraps raw nonce bytes issued by the Server.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    /// The raw nonce bytes, for verifier computation only.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

impl fmt::Debug for PairingNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingNonce(REDACTED)")
    }
}

impl Serialize for PairingNonce {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut hex = String::with_capacity(Self::HEX_LENGTH);
        for byte in self.0 {
            use fmt::Write;
            write!(hex, "{byte:02x}").expect("string write");
        }
        serializer.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for PairingNonce {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NonceVisitor;

        impl Visitor<'_> for NonceVisitor {
            type Value = PairingNonce;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("64 lowercase hex characters")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.len() != PairingNonce::HEX_LENGTH {
                    return Err(E::custom(
                        "pairing nonce must be exactly 64 lowercase hex characters",
                    ));
                }
                let mut bytes = [0u8; PairingNonce::LENGTH];
                let (pairs, _) = value.as_bytes().as_chunks::<2>();
                for (slot, &[high, low]) in bytes.iter_mut().zip(pairs) {
                    let high =
                        hex_value(high).ok_or_else(|| E::custom("invalid pairing nonce hex"))?;
                    let low =
                        hex_value(low).ok_or_else(|| E::custom("invalid pairing nonce hex"))?;
                    *slot = (high << 4) | low;
                }
                Ok(PairingNonce(bytes))
            }
        }

        deserializer.deserialize_str(NonceVisitor)
    }
}

/// The Client's authenticated negotiation request — the first and only
/// message that may open an execution binding.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    /// Wire protocol revision; must equal [`PROTOCOL_VERSION`].
    pub version: u16,
    /// The Session this binding belongs to.
    pub session: SessionId,
    /// The logical execution environment.
    pub environment: EnvironmentId,
    /// The physical Client instance asserting the binding.
    pub instance: ClientInstanceId,
    /// The binding generation asserted for this instance.
    pub generation: Generation,
    /// The exact normalized absolute workspace the Client claims.
    pub workspace: PathBuf,
    /// The capabilities the Client claims to physically own.
    pub capabilities: BTreeSet<Capability>,
    /// The binding credential issued to this reservation.
    pub credential: BindingCredential,
}

impl fmt::Debug for Hello {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hello")
            .field("version", &self.version)
            .field("session", &self.session)
            .field("environment", &self.environment)
            .field("instance", &self.instance)
            .field("generation", &self.generation)
            .field("workspace", &self.workspace)
            .field("capabilities", &self.capabilities)
            .field("credential", &self.credential)
            .finish()
    }
}

/// The Server's acceptance of a [`Hello`]: this authenticated connection is
/// now the one live connection of the binding.
///
/// An admission proves connection ownership only. It does not report
/// workspace, configuration, MCP, or Python readiness, and it does not
/// grant dispatch by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    /// The negotiated protocol revision — always [`PROTOCOL_VERSION`].
    pub version: u16,
    /// The process-wide Server incarnation operations are keyed under.
    pub server_incarnation: crate::process::ServerIncarnation,
    /// The bound Session.
    pub session: SessionId,
    /// The bound environment.
    pub environment: EnvironmentId,
    /// The bound Client instance.
    pub instance: ClientInstanceId,
    /// The bound generation.
    pub generation: Generation,
    /// The exact workspace the binding is fixed to.
    pub workspace: PathBuf,
}

/// Readiness of the Client's Workspace Python owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspacePythonStatus {
    /// This binding has scheduled no preparation.
    Unscheduled,
    /// A supervised probe or preparation is still running.
    Preparing,
    /// The current Workspace inputs have a verified interpreter.
    Ready,
    /// No current verified interpreter exists.
    Unavailable,
}

/// The reason a [`Hello`] was rejected.
///
/// Wire-stable machine codes. A reject never echoes the submitted credential
/// or any other secret-bearing field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "kebab-case")]
pub enum Reject {
    /// The protocol revision is not spoken; there is no implicit downgrade.
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    /// The Session does not match this reservation.
    #[error("session mismatch")]
    SessionMismatch,
    /// The environment does not match this reservation.
    #[error("environment mismatch")]
    EnvironmentMismatch,
    /// The Client instance does not match this reservation.
    #[error("instance mismatch")]
    InstanceMismatch,
    /// The generation does not match this reservation.
    #[error("generation mismatch")]
    GenerationMismatch,
    /// The workspace is not exactly the reserved normalized absolute path.
    #[error("workspace mismatch")]
    WorkspaceMismatch,
    /// A required capability is absent from the Hello.
    #[error("missing required capability")]
    MissingCapabilities,
    /// The credential verifier did not match.
    #[error("invalid credential")]
    InvalidCredential,
    /// A different connection is already live on this binding.
    #[error("binding already has a live connection")]
    AlreadyConnected,
    /// The binding detached; re-admission requires live-instance recovery
    /// proof that this layer does not provide (#577).
    #[error("binding detached; recovery proof required")]
    RecoveryRequired,
    /// The binding was revoked and can never be dispatched again.
    #[error("binding revoked")]
    Revoked,
    /// The control lane's pairing nonce matched no pending attempt —
    /// missing, expired, mismatched, or already consumed.
    #[error("invalid pairing")]
    InvalidPairing,
}

/// The role an accepted connection asserts with its first record.
///
/// Every admitted connection begins with exactly one of these: the data
/// lane — the dispatch-capable half — presents the authenticated
/// [`Hello`] it always did; the control lane presents a [`ControlHello`]
/// carrying the same identity plus the [`PairingNonce`] challenge the
/// data lane's admission issued. The two halves of a binding share one
/// authenticated pair: either lane alone is never dispatchable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PeerHello {
    /// The data-lane negotiation request.
    Data(Hello),
    /// The control-lane pairing request.
    Control(ControlHello),
}

/// The control lane's authenticated pairing request — every identity
/// field of the data lane's [`Hello`] (capabilities excluded: the
/// control lane never dispatches) plus the [`PairingNonce`] the data
/// lane's admission delivered. A nonce matching no pending pairing, a
/// stale one, or a replayed one is refused.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlHello {
    /// Wire protocol revision; must equal [`PROTOCOL_VERSION`].
    pub version: u16,
    /// The Session this binding belongs to.
    pub session: SessionId,
    /// The logical execution environment.
    pub environment: EnvironmentId,
    /// The physical Client instance asserting the binding.
    pub instance: ClientInstanceId,
    /// The binding generation asserted for this instance.
    pub generation: Generation,
    /// The exact normalized absolute workspace the Client claims.
    pub workspace: PathBuf,
    /// The binding credential issued to this reservation.
    pub credential: BindingCredential,
    /// The pairing challenge the data lane's [`DataAdmission`] issued.
    pub connection_nonce: PairingNonce,
}

impl fmt::Debug for ControlHello {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlHello")
            .field("version", &self.version)
            .field("session", &self.session)
            .field("environment", &self.environment)
            .field("instance", &self.instance)
            .field("generation", &self.generation)
            .field("workspace", &self.workspace)
            .field("credential", &self.credential)
            .field("connection_nonce", &self.connection_nonce)
            .finish()
    }
}

/// The Server's acceptance of a data-lane [`Hello`]: the half-admission
/// plus the [`PairingNonce`] challenge the Client must echo on the
/// control lane it connects next. The binding is not live — nothing may
/// dispatch — until the pairing commits.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataAdmission {
    /// The reserved identity this pair binds.
    pub admission: Admission,
    /// The fresh single-attempt pairing challenge.
    pub challenge: PairingNonce,
}

impl fmt::Debug for DataAdmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataAdmission")
            .field("admission", &self.admission)
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// The Server's reply to a data-lane [`Hello`]: the challenge-carrying
/// half-admission or a typed refusal.
pub type DataHelloReply = Result<DataAdmission, Reject>;

/// The Server's reply to a [`ControlHello`]: the committed pair's
/// admission or a typed refusal.
pub type ControlHelloReply = Result<Admission, Reject>;

/// One command the Server delivers to the Client on the control lane.
///
/// The control lane is independently authenticated and never a dispatch
/// channel: it carries only health probes, live-mutation commit
/// cancellation, and terminal revocation. Exactly one request is
/// outstanding per pair at a time — the next [`ControlPoll`] carries
/// its [`ControlReply`] as `receipt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ControlRequest {
    /// A liveness probe: the Client echoes `nonce` in `Healthy`.
    Health {
        /// The fresh nonce the reply must echo.
        nonce: DecimalU64,
    },
    /// Cancels the live mutation commit for `ticket`: the publish
    /// crossing is prevented when it has not happened yet, and the
    /// honest disposition is reported either way.
    CancelMutation {
        /// The ticket whose commit must not cross.
        ticket: DecimalU64,
    },
    /// Terminally revokes the binding: the Client fences every local
    /// commit — a commit that has not crossed never publishes — and
    /// acknowledges with `Revoked`.
    Revoke,
    /// Calls for physical native observer cleanup.
    StopWatch {
        /// The exact registered watch incarnation.
        id: crate::watch::WatchId,
    },
    /// Cancels one live process operation: a `CancelProcess` delivered
    /// before physical START must admit zero spawn; a later one awaits
    /// actual physical containment before the honest disposition.
    CancelProcess {
        /// The complete operation identity to cancel.
        key: crate::process::ProcessOperationKey,
    },
}

/// What the Client reports for a [`ControlRequest::CancelMutation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum CancelDisposition {
    /// The ticket existed and the publish provably never crossed: the
    /// private stage was dropped unpublished.
    CancelledBeforeCommit,
    /// The ticket existed but the publish had already crossed when the
    /// cancellation was arbitrated.
    AlreadyCrossed,
    /// No live or just-crossed ticket matches — the cancellation had no
    /// target. Never means "no effect" on a real ticket.
    Unknown,
}

/// What the Client reports for a [`ControlRequest::CancelProcess`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProcessCancelDisposition {
    /// The signal arrived before physical START: zero spawn admitted,
    /// the registration tombstoned `Cancelled`.
    CancelledBeforeStart,
    /// The operation was at or past START: cancellation was delivered and
    /// the physical worker is performing real containment — completion is
    /// observed through the operation's settled state, never claimed here.
    CancellationRequested,
    /// The operation had already settled — cancellation had no target.
    AlreadySettled,
    /// The key had no matching record or its physical settlement is unconfirmed.
    Unknown,
}

/// The Client's receipt for one delivered [`ControlRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ControlReply {
    /// The `Health` nonce echoed — fresh proof, never a replayed one.
    Healthy {
        /// The echoed nonce.
        nonce: DecimalU64,
    },
    /// The `CancelMutation` outcome for the exact ticket.
    MutationCancelled {
        /// The ticket the disposition reports.
        ticket: DecimalU64,
        /// What arbitration proved.
        disposition: CancelDisposition,
    },
    /// The `Revoke` was arbitrated: local commits are fenced.
    Revoked,
    /// The observer stop disposition.
    WatchStopped {
        /// The exact requested watch incarnation.
        id: crate::watch::WatchId,
        /// Physical closure was observed or the watch no longer exists.
        settled: bool,
    },
    /// The `CancelProcess` outcome for the exact operation key.
    ProcessCancelled {
        /// The exact supplied operation key.
        key: crate::process::ProcessOperationKey,
        /// What arbitration proved.
        disposition: ProcessCancelDisposition,
    },
}

/// The Client's turn on the control lane: one continuous poll that
/// acknowledges the previously delivered request and offers a fresh
/// health nonce the Server must echo in its [`ControlTurn`].
///
/// The lane is client-paced turn-taking: the Server answers each poll
/// with at most one request, so an idle lane exchanges one bounded pair
/// per poll period and a delivered command's receipt arrives on the
/// poll right after it — control never waits behind data traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlPoll {
    /// The poll's own sequence, strictly increasing from 1.
    pub sequence: DecimalU64,
    /// The receipt for the request the previous `ControlTurn`
    /// delivered, `None` when that turn carried none.
    pub receipt: Option<ControlReply>,
    /// The fresh client nonce the `ControlTurn` must echo.
    pub health: DecimalU64,
}

/// The Server's answer to one [`ControlPoll`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlTurn {
    /// Echo of the answered poll's sequence.
    pub sequence: DecimalU64,
    /// Echo of the poll's `health` — the Server's fresh liveness proof.
    pub health: DecimalU64,
    /// The queued command this turn delivers, if any.
    pub request: Option<ControlRequest>,
}

/// A filesystem mutation with all paths relative to one admitted workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum TreeMutation {
    /// Create a directory and optionally its missing ancestors.
    CreateDirectory { path: PathBuf, recursive: bool },
    /// Remove an entry. Force permits a missing entry.
    Remove {
        path: PathBuf,
        recursive: bool,
        force: bool,
    },
    /// Copy a file or directory without overwriting an existing entry.
    Copy {
        source: PathBuf,
        destination: PathBuf,
        recursive: bool,
    },
}

/// The Server's one typed request on a live execution connection.
///
/// The Server is the only initiator; the Client answers every request with
/// exactly one [`ClientReply`]. `Ping`/`Pong` is the liveness control flow;
/// `ReadRange`, `GetMetadata`, `ReadDirectory`, and the ticketed
/// `PrepareWrite`/`CommitWrite`/`AbortWrite` sequence are the implemented
/// physical operations — there is no raw or arbitrary message path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ClientRequest {
    /// A liveness probe.
    Ping,
    /// One managed-process operation request — register, start, query,
    /// ordered output paging, stdin, resize, close, release.
    Process(crate::process::ProcessRequest),
    /// Read lines of a Client-owned Bash continuation.
    ReadBashOutput {
        /// The operation under the current binding.
        key: crate::process::ProcessOperationKey,
        /// The Conversation making the read.
        conversation: String,
        /// One-based line offset.
        offset: DecimalU64,
        /// Optional positive line limit.
        limit: Option<DecimalU64>,
    },
    /// Search one Client-owned Bash continuation.
    SearchBashOutput {
        /// The operation under the current binding.
        key: crate::process::ProcessOperationKey,
        /// The Conversation making the search.
        conversation: String,
        /// The bounded search parameters.
        spec: SearchSpec,
    },
    /// One optional-save request on a binding offering managed output.
    Save(crate::save::SaveRequest),
    /// Read lines of a published Client-owned import.
    ReadSavedOutput {
        /// The published locator under this binding.
        locator: crate::save::Locator,
        /// The Conversation making the read.
        conversation: crate::save::ConversationId,
        /// One-based line offset.
        offset: DecimalU64,
        /// Optional positive line limit.
        limit: Option<DecimalU64>,
    },
    /// Search one published Client-owned import.
    SearchSavedOutput {
        /// The published locator under this binding.
        locator: crate::save::Locator,
        /// The Conversation making the search.
        conversation: crate::save::ConversationId,
        /// The bounded search parameters.
        spec: SearchSpec,
    },
    /// A bounded revision-checked range read under the binding workspace.
    ReadRange {
        /// The workspace-relative file path: normal components only.
        relative_path: PathBuf,
        /// The byte offset to start at.
        offset: DecimalU64,
        /// At most [`MAX_RAW_CHUNK_BYTES`] bytes.
        max_bytes: u32,
        /// The revision the file must still hold, when checked.
        expected_revision: Option<String>,
    },
    /// The metadata of one entry under the binding workspace, stat'ed
    /// without following a final symlink.
    GetMetadata {
        /// The workspace-relative entry path: normal components only, or
        /// empty for the workspace root itself.
        relative_path: PathBuf,
    },
    /// One bounded page of a directory under the binding workspace.
    ReadDirectory {
        /// The workspace-relative directory path: normal components only,
        /// or empty for the workspace root itself.
        relative_path: PathBuf,
        /// The entry index the page starts at.
        offset: u32,
        /// At most [`MAX_DIRECTORY_PAGE_ENTRIES`] entries.
        limit: u32,
        /// The revision the directory must still hold, when checked.
        expected_revision: Option<String>,
    },
    /// A bounded mutation beneath the admitted workspace.
    MutateTree(TreeMutation),
    /// Attempt-scoped Workspace baseline, capture, and comparison.
    WorkspaceChanges(changes::ChangeRequest),
    /// Schedule Client-owned Python readiness for this admitted Workspace.
    PrepareWorkspacePython,
    /// Read the verified preparation state of this admitted Workspace.
    WorkspacePythonStatus,
    /// Register a native observer or receive its bounded coalesced hints.
    Watch(crate::watch::WatchRequest),
    /// First phase of one conditional write: retain the exact named target
    /// under the binding workspace, verify the expected original, and stage
    /// the payload privately. Nothing named by `relative_path` mutates until
    /// the matching `CommitWrite` — the base64 payload decodes to at most
    /// [`MAX_RAW_CHUNK_BYTES`].
    PrepareWrite {
        /// The exact named target: strictly normalized relative path below
        /// the binding workspace whose leaf is the file to write. Missing
        /// parents are created under a private staging subtree at commit.
        relative_path: PathBuf,
        /// What the named leaf must be at retain time.
        expectation: WriteExpectation,
        /// The full payload, base64-encoded; never staged raw. Payloads
        /// over [`MAX_RAW_CHUNK_BYTES`] arrive through
        /// `BeginWrite`/`WriteChunk` instead.
        data_base64: String,
    },
    /// The chunked first phase of one conditional write: retains the
    /// target and opens a live ticket whose planned bytes then arrive in
    /// [`MAX_RAW_CHUNK_BYTES`] `WriteChunk` exchanges at deterministic
    /// absolute offsets. `total` is the declared complete length, at
    /// most [`MAX_WRITE_BYTES`].
    BeginWrite {
        /// The exact named target, as on `PrepareWrite`.
        relative_path: PathBuf,
        /// What the named leaf must be at retain time.
        expectation: WriteExpectation,
        /// The declared complete payload length in bytes.
        total: DecimalU64,
    },
    /// One bounded payload chunk for the live ticketed `BeginWrite`
    /// preparation: `offset` must equal the bytes received so far —
    /// chunks append deterministically, never out of order.
    WriteChunk {
        /// The exact ticket the chunk feeds.
        ticket: DecimalU64,
        /// The absolute byte offset this chunk lands at.
        offset: DecimalU64,
        /// The chunk, base64-encoded; never staged raw.
        data_base64: String,
    },
    /// Second phase: publish the staged payload of the one live
    /// `WritePrepared` ticket on this connection. The publish crossing is
    /// irreversible; a consumed ticket can never commit again.
    CommitWrite {
        /// The exact ticket returned by the live `WritePrepared` reply.
        ticket: DecimalU64,
    },
    /// Releases an issued ticket on this connection. An acknowledged
    /// repeat proves that its preparation is absent.
    AbortWrite {
        /// The exact ticket returned by the live `WritePrepared` reply.
        ticket: DecimalU64,
    },
    /// First phase of one conditional edit: retain the exact named
    /// original, verify the required revision, apply the bounded
    /// text-edit algorithm to it, and stage the result privately.
    /// Nothing named by `relative_path` mutates until the matching
    /// `CommitWrite`; the same one-live-ticket budget as `PrepareWrite`
    /// applies and a semantic failure is `EditRefused`, never a stage.
    PrepareEdit {
        /// The exact named target: strictly normalized relative path below
        /// the binding workspace whose leaf is the file to edit.
        relative_path: PathBuf,
        /// What the named leaf must be at retain time. An edit always
        /// names an existing original: `Absent` is malformed.
        expectation: WriteExpectation,
        /// At most [`MAX_EDIT_REPLACEMENTS`] replacements whose aggregate
        /// old+new bytes stay under [`MAX_EDIT_REQUEST_BYTES`].
        replacements: Vec<TextReplacement>,
    },
    /// One bounded logical-text page of a file under the binding
    /// workspace: the complete-line page contract the native Read tool
    /// projects, carried out by the streaming scanner. The whole source is
    /// validated and counted even past the returned window.
    ReadLines {
        /// The workspace-relative file path: normal components only.
        relative_path: PathBuf,
        /// The 1-based line the page starts at; must be nonzero.
        offset: DecimalU64,
        /// The requested line bound, when given; never zero.
        limit: Option<DecimalU64>,
        /// The revision the file must still hold, when checked.
        expected_revision: Option<String>,
    },
    /// One bounded logical-text page of a whitelisted structured document
    /// under the binding workspace: the Client decodes the source with
    /// the shared rustX parser policy — never a caller-supplied MIME type
    /// or URI — and serves the complete-line projection of the decoded
    /// Markdown. The document format is chosen by the Client from the
    /// path's extension against the rustX whitelist; every other source
    /// is [`ReadError::UnsupportedDocument`].
    ReadDocument {
        /// The workspace-relative document path: normal components only.
        relative_path: PathBuf,
        /// The 1-based line the page starts at; must be nonzero.
        offset: DecimalU64,
        /// The requested line bound, when given; never zero.
        limit: Option<DecimalU64>,
        /// The revision the file must still hold, when checked.
        expected_revision: Option<String>,
    },
    /// One bounded workspace search: the shared file-universe traversal
    /// rooted at `relative_root` — the workspace root itself when empty —
    /// narrowed by `spec` into one selected-records page. The universe is
    /// descriptor-relative the whole way: hidden and ignored files are
    /// visible, symlinks and special entries are never part of it, and no
    /// unselected file body ever crosses the wire.
    Search {
        /// The workspace-relative search root: normal components only, or
        /// empty for the workspace root itself.
        relative_root: PathBuf,
        /// The kind of search and its bounds.
        spec: SearchSpec,
    },
}

/// The kind of one bounded `Search` request and its wire bounds.
///
/// Both variants enumerate the same shared file universe; the pattern and
/// optional `glob` filter only narrow it — they never widen it. The
/// caller is the sole authority for compiling the pattern for
/// diagnostics; the Client compiles the same text for the physical match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum SearchSpec {
    /// Select the universe files whose root-relative path matches
    /// `pattern`, in global bytewise lexical order. The root must be a
    /// directory — a file root is [`ReadError::SearchNeedsDirectory`].
    Glob {
        /// The glob, at most [`MAX_SEARCH_PATTERN_BYTES`] bytes.
        pattern: String,
        /// The selected-record bound; never zero.
        limit: DecimalU64,
    },
    /// Select the matching lines of every universe file — narrowed by
    /// `glob`, when present — with `context` lines on each side. The root
    /// may name one file, which is then the whole universe.
    Grep {
        /// The regex — or literal text when `literal` — at most
        /// [`MAX_SEARCH_PATTERN_BYTES`] bytes.
        pattern: String,
        /// The file filter, at most [`MAX_SEARCH_PATTERN_BYTES`] bytes.
        glob: Option<String>,
        /// Match `pattern` literally rather than as a regex.
        literal: bool,
        /// Match without regard to case.
        ignore_case: bool,
        /// Surrounding lines on each side, at most
        /// [`MAX_SEARCH_CONTEXT_LINES`].
        context: u32,
        /// The selected-match bound; never zero.
        limit: DecimalU64,
    },
}

impl std::fmt::Display for EditRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyEdits => f.write_str("empty edits"),
            Self::EmptyAnchor { index } => write!(f, "empty anchor at index {index}"),
            Self::Missing { index } => write!(f, "missing anchor at index {index}"),
            Self::Ambiguous { index, occurrences } => {
                write!(
                    f,
                    "ambiguous anchor at index {index}: {occurrences} occurrences"
                )
            }
            Self::Overlap { first, second } => {
                write!(f, "overlapping edits {first} and {second}")
            }
            Self::UnsafeNormalization { index } => {
                write!(f, "unsafe fuzzy normalization at index {index}")
            }
            Self::NoChange => f.write_str("edits produced identical content"),
            Self::TooLarge => f.write_str("edit exceeds a bounded resource"),
        }
    }
}

impl std::error::Error for EditRefusal {}

/// One typed `oldText` → `newText` replacement on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TextReplacement {
    /// The anchor text, matched exactly first and fuzzily on a miss.
    pub old_text: String,
    /// The replacement text.
    pub new_text: String,
}

/// The stable typed reason an edit failed — the `PrepareEdit` refusal
/// on the wire and, re-exported as `EditFailure`, the pure kernel's
/// own public failure type. The Server renders user-facing wording
/// from the request's path and counts — the refusal itself never
/// carries a path or anchor text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum EditRefusal {
    /// The request carried no replacements.
    EmptyEdits,
    /// `replacements[index]` carried an empty `old_text` anchor.
    EmptyAnchor { index: u32 },
    /// `replacements[index]`'s anchor matched nowhere in the original.
    Missing { index: u32 },
    /// `replacements[index]`'s anchor matched `occurrences` places.
    Ambiguous { index: u32, occurrences: u32 },
    /// The planned ranges of `first` and `second` overlap in the
    /// original snapshot.
    Overlap { first: u32, second: u32 },
    /// `replacements[index]`'s fuzzy match cannot map to one safe
    /// original range.
    UnsafeNormalization { index: u32 },
    /// The edits produced byte-identical content.
    NoChange,
    /// A bounded resource was exceeded: source or output over
    /// `MAX_EDIT_FILE_BYTES`, a request over its count or byte cap, or
    /// a fuzzy projection over its provenance bound.
    TooLarge,
}

/// The Client's one typed reply to a [`ClientRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ClientReply {
    /// The reply to a `Ping`.
    Pong,
    /// The reply to a `Process` request.
    Process(crate::process::ProcessReply),
    /// The reply to a `Save` request.
    Save(crate::save::SaveReply),
    /// The verified range the Client physically read.
    Range {
        /// The returned bytes, base64. Never exceeds `max_bytes`.
        data_base64: String,
        /// The offset the bytes start at — echoes the request.
        offset: DecimalU64,
        /// The next offset to read, absent at end of file.
        next_offset: Option<DecimalU64>,
        /// The source-descriptor stamp of the file the bytes came from.
        metadata: FileStamp,
    },
    /// The kind and stamp of the queried entry.
    Metadata {
        /// The entry's kind and stamp, stat'ed without following it.
        metadata: EntryStamp,
    },
    /// One page of a directory listing.
    Directory {
        /// The revision the listing was verified under.
        revision: String,
        /// The page's entries in the directory's stable name order.
        entries: Vec<PageEntry>,
        /// The index the next page starts at, absent at the end.
        next_offset: Option<u32>,
    },
    /// The requested tree mutation finished.
    TreeMutated,
    /// Reply from the Client-owned Workspace observation service.
    WorkspaceChanges(changes::ChangeReply),
    /// Physical preparation was scheduled, or was already in flight.
    WorkspacePythonScheduled,
    /// Current physical readiness, independent of authentication.
    WorkspacePythonStatus(WorkspacePythonStatus),
    /// A change crossed and the tree operation could not settle successfully.
    TreeOutcomeUnknown,
    /// Registration receipt or coalesced hints from one native observer.
    Watch(crate::watch::WatchReply),
    /// The verified logical-text page the Client physically scanned.
    Lines {
        /// The source-descriptor stamp of the file the page came from.
        metadata: FileStamp,
        /// The shown text: at most [`MAX_TEXT_BYTES`] joined bytes.
        text: String,
        /// The addressable line count of the whole source.
        total_lines: DecimalU64,
        /// The shown element count of this page.
        shown_lines: u32,
        /// Why the page ended.
        end: LineEnd,
    },
    /// A `PrepareWrite` staged privately and verified: the ticket is a
    /// Client-assigned nonzero scalar unique to this connection, `original`
    /// is the retained target's verified stamp, `None` for a create-only
    /// target. At most one ticket is live per connection at a time.
    WritePrepared {
        ticket: DecimalU64,
        original: Option<FileStamp>,
    },
    /// A `CommitWrite` published and verified: the returned stamp describes
    /// the retained published descriptor, never a path reopened by name.
    Written {
        ticket: DecimalU64,
        metadata: FileStamp,
    },
    /// An `AbortWrite` confirmed the issued ticket has no preparation.
    WriteAborted { ticket: DecimalU64 },
    /// A `WriteChunk` landed at the ticket's next absolute offset.
    WriteChunkReceived {
        ticket: DecimalU64,
        offset: DecimalU64,
    },
    /// The `CommitWrite` publish crossing already happened but a later
    /// stage failed: the named outcome is unknowable and must be reported
    /// as such — never as a pre-commit denial.
    CommitUnknown { ticket: DecimalU64 },
    /// The `CommitWrite` was arbitrated cancelled before its publish
    /// crossed: the private stage was dropped unpublished. The ticket is
    /// consumed; nothing named by it was written.
    WriteCancelled { ticket: DecimalU64 },
    /// A `PrepareEdit` staged privately and verified: `ticket` is a
    /// Client-assigned nonzero scalar unique to this connection,
    /// `original` is the retained source's verified stamp (an edit always
    /// names an existing file), and `published_size` is the exact byte
    /// length the later `Written` must report. At most one ticket is live
    /// per connection at a time.
    EditPrepared {
        ticket: DecimalU64,
        original: FileStamp,
        published_size: DecimalU64,
    },
    /// A `PrepareEdit` refused for one stable typed semantic reason —
    /// the request was well-formed but the edits did not apply. No stage
    /// exists and the connection stays live.
    EditRefused { reason: EditRefusal },
    /// The verified logical-text page the Client decoded from a
    /// whitelisted document: `source` stamps the physical document the
    /// page came from, `logical_bytes` is the full decoded Markdown
    /// length — independent of the compressed source's size — and the
    /// remaining fields carry the same complete-line contract as
    /// `Lines`.
    DocumentPage {
        /// The source-descriptor stamp of the document the page came
        /// from.
        source: FileStamp,
        /// The byte length of the complete decoded Markdown text.
        logical_bytes: DecimalU64,
        /// The shown text: at most [`MAX_TEXT_BYTES`] joined bytes.
        text: String,
        /// The addressable line count of the whole decoded text.
        total_lines: DecimalU64,
        /// The shown element count of this page.
        shown_lines: u32,
        /// Why the page ended.
        end: LineEnd,
    },
    /// The bounded search's selected records: `body` is the already
    /// projected record text — newline-joined tool records only, never
    /// notices — and `matches` counts the selected primary records. The
    /// limit flags are proofs, not hints: `hit_limit` asserts at least
    /// one further match was seen and excluded past `matches` (which
    /// then equals the request's bound), and `byte_limit` asserts the
    /// joined-body budget stopped the selection. `short_lines` reports
    /// at least one grep record preview shortened to
    /// [`SEARCH_PREVIEW_SCALARS`] scalars — always false for a glob.
    SearchPage {
        /// The selected records, at most [`MAX_TEXT_BYTES`] joined bytes.
        body: String,
        /// The selected primary record count — matched files for a glob,
        /// matched lines for a grep.
        matches: DecimalU64,
        /// At least one match was excluded because `matches` reached the
        /// requested bound.
        hit_limit: bool,
        /// The joined-body byte budget stopped the selection.
        byte_limit: bool,
        /// At least one record preview was shortened — grep only.
        short_lines: bool,
    },
    /// The request failed; `kind` carries only the stable reason.
    Error {
        /// Why the operation failed — never a raw path or payload.
        kind: ReadError,
    },
}

/// The source-descriptor stamp of one filesystem entry.
///
/// Carried on every physical reply so the Server can project the result
/// without re-statting the workspace itself: a general entry stamp of
/// size, content-stable revision, `dev:ino` identity, and modification
/// time. On a `Range` reply the wire file type is implicitly regular —
/// a `Range` only ever describes regular-file bytes; queries report
/// the kind explicitly through [`EntryStamp`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileStamp {
    /// The entry's byte length at the query.
    pub size: DecimalU64,
    /// The content-stable revision stamp the entry was verified under.
    pub revision: String,
    /// The `dev:ino` identity of the entry.
    pub identity: String,
    /// Modification time in milliseconds since the Unix epoch, when known.
    pub modified_at_ms: Option<i64>,
}

/// The kind of one filesystem entry reported by a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryKind {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// A symbolic link — reported, never followed.
    Symlink,
    /// Any other entry type (fifo, socket, device node).
    Other,
}

/// One filesystem entry's kind plus its source-descriptor stamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryStamp {
    /// What the entry is.
    pub kind: EntryKind,
    /// The entry's size, revision, `dev:ino` identity, and mtime.
    pub stamp: FileStamp,
}

/// Why one `Lines` page ended — the wire mirror of the physical scanner's
/// page-end reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum LineEnd {
    /// The selected window ran to the end of the source.
    Complete,
    /// The caller's `limit` ended the selection while lines remained.
    RequestedLimit,
    /// The complete-line count reached [`MAX_TEXT_LINES`].
    LineLimit,
    /// The joined bytes reached [`MAX_TEXT_BYTES`].
    ByteLimit,
    /// The first selected line alone exceeds [`MAX_TEXT_BYTES`].
    FirstLineTooLarge {
        /// Its full byte length, even unstaged.
        bytes: DecimalU64,
    },
    /// The requested offset addresses a line beyond the source.
    OffsetPastEnd,
}

/// One entry of a directory page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEntry {
    /// The entry name verbatim: exactly one component — never `/`, `.`,
    /// or `..`. Any other byte is ordinary literal name data, control
    /// characters included (the pre-migration product contract returned
    /// names unchanged); the Server never joins a name into a path it
    /// opens, so no byte can escape through it.
    pub name: String,
    /// The entry's kind and stamp, stat'ed without following it.
    pub metadata: EntryStamp,
}

/// The stable machine reason a physical read failed.
///
/// A wire code only: it never carries a raw path, credential, or payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadError {
    /// The request was malformed or out of bounds.
    Invalid,
    /// The path escapes the workspace or is otherwise not permitted.
    Denied,
    /// The path does not exist.
    NotFound,
    /// The file changed across the read or no longer holds the expected
    /// revision.
    Conflict,
    /// The source is not UTF-8 — binary content is never fabricated as
    /// text.
    InvalidUtf8,
    /// The path's extension is not in the rustX document whitelist, so no
    /// structured decode exists for it — a distinct unavailability, never
    /// conflated with a missing capability or a parser failure.
    UnsupportedDocument,
    /// The document source exceeds [`MAX_DOCUMENT_SOURCE_BYTES`]; it is
    /// refused before any decode work starts.
    DocumentTooLarge,
    /// The document's parser rejected the source; the reason is never
    /// carried on the wire.
    DocumentDecodeFailed,
    /// The document decoded but has no extractable text layer — OCR and
    /// image inference never substitute for one.
    NoTextLayer,
    /// A `Search` exhausted one of its wire traversal or working budgets —
    /// depth, entries, retained paths, directory names, or the grep
    /// buffer — and fails closed rather than publishing a partial
    /// universe.
    SearchBudgetExceeded,
    /// A `Search` glob root named a file — its universe is directories
    /// only.
    SearchNeedsDirectory,
    /// The read could not be completed for another reason.
    Unavailable,
    /// The connection's bounded preparation budget is full — the caller
    /// may retry after another ticket is consumed.
    Busy,
}

/// The transport endpoint a [`Bootstrap`] binds the Client to.
///
/// One binding publishes exactly one mode: the owner-only Unix socket for
/// the local sibling composition, or the validated WebSocket URL for an
/// operator-provisioned remote Client. Endpoint *shape* is validated at
/// use time by `rustx-execution-transport` — this type is the serde DTO
/// only and performs no I/O.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase", deny_unknown_fields)]
pub enum BootstrapEndpoint {
    /// The Unix socket the Client connects to.
    Unix {
        /// Absolute path of the Server's listening socket.
        socket_path: PathBuf,
    },
    /// The authenticated WebSocket execution endpoint.
    WebSocket {
        /// The exact `wss://IP:PORT/execution/v2` URL the Client connects to.
        url: String,
        /// Base64 DER of the binding-specific loopback TLS certificate.
        /// An operator-managed public endpoint instead uses its own CA.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        certificate_der: Option<String>,
    },
}

/// The server-published bootstrap bundle for one local binding.
///
/// It is delivered out of band as exactly one framed record in an
/// owner-only file: the Client learns the socket endpoint and every
/// identity it must assert — including its credential — from it. The
/// bundle deliberately carries no capabilities: the Client constructs its
/// Hello with the fixed capabilities it actually implements, so a tampered
/// file can never advertise features the Client does not have.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    /// The endpoint the Client connects to.
    pub endpoint: BootstrapEndpoint,
    /// The protocol revision the Server expects.
    pub version: u16,
    /// The process-wide Server incarnation that assigned this binding.
    pub server_incarnation: crate::process::ServerIncarnation,
    /// The Session the binding belongs to.
    pub session: SessionId,
    /// The logical execution environment.
    pub environment: EnvironmentId,
    /// The physical Client instance that must assert the binding.
    pub instance: ClientInstanceId,
    /// The binding generation the instance must assert.
    pub generation: Generation,
    /// The exact normalized absolute workspace the binding fixes.
    pub workspace: PathBuf,
    /// The binding credential issued to this reservation.
    pub credential: BindingCredential,
}

/// Whether `path` is a normalized absolute path.
///
/// The check is purely lexical — it never touches the filesystem. Root plus
/// normal components only: no `.`, `..`, repeated separators, or trailing
/// separators. This is the workspace form the binding contract fixes.
#[must_use]
pub fn is_normalized_absolute_path(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::CurDir | Component::ParentDir) {
            return false;
        }
        normalized.push(component.as_os_str());
    }
    // `Path` equality is component-wise, so the comparison must be on the
    // exact lexical form to reject redundant separators and trailing `/`.
    normalized.as_os_str() == path.as_os_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential() -> BindingCredential {
        BindingCredential::from_bytes([0x5a; BindingCredential::LENGTH])
    }

    fn hello() -> Hello {
        Hello {
            version: PROTOCOL_VERSION,
            session: SessionId::new("session-1"),
            environment: EnvironmentId::new("env-1"),
            instance: ClientInstanceId::new("instance-1"),
            generation: Generation::try_from(1).unwrap(),
            workspace: PathBuf::from("/work/root"),
            capabilities: BTreeSet::from([Capability::Files, Capability::Process]),
            credential: credential(),
        }
    }

    #[test]
    fn decimal_u64_accepts_canonical_decimals_including_zero() {
        for good in ["0", "1", "42", "18446744073709551615"] {
            assert!(good.parse::<DecimalU64>().is_ok(), "{good}");
            assert_eq!(
                serde_json::from_str::<DecimalU64>(&format!("\"{good}\""))
                    .unwrap()
                    .get(),
                good.parse::<u64>().unwrap()
            );
        }
        for bad in [
            "",
            "00",
            "01",
            "+1",
            "-1",
            " 1",
            "1 ",
            "1e3",
            "abc",
            "18446744073709551616",
        ] {
            assert!(bad.parse::<DecimalU64>().is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_string(&DecimalU64::new(0)).unwrap(), "\"0\"");
        assert!(serde_json::from_str::<DecimalU64>("0").is_err());
    }

    #[test]
    fn generation_accepts_only_canonical_positive_decimals() {
        for good in ["1", "7", "42", "18446744073709551615"] {
            assert!(good.parse::<Generation>().is_ok(), "{good}");
        }
        for bad in [
            "",
            "0",
            "00",
            "01",
            "007",
            "+1",
            "-1",
            " 1",
            "1 ",
            "1_0",
            "1e3",
            "abc",
            "18446744073709551616",
        ] {
            assert!(bad.parse::<Generation>().is_err(), "{bad}");
        }
    }

    #[test]
    fn generation_wire_is_a_decimal_string() {
        let generation = Generation::try_from(7).unwrap();
        assert_eq!(serde_json::to_string(&generation).unwrap(), "\"7\"");
        assert!(serde_json::from_str::<Generation>("7").is_err());
        assert!(serde_json::from_str::<Generation>("\"07\"").is_err());
        assert_eq!(
            serde_json::from_str::<Generation>("\"7\"").unwrap(),
            generation
        );
    }

    #[test]
    fn credential_wire_is_exactly_64_lowercase_hex() {
        let credential = credential();
        let json = serde_json::to_string(&credential).unwrap();
        assert_eq!(json.len(), 66);
        let decoded: BindingCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.as_bytes(), credential.as_bytes());

        let mut bad = Vec::new();
        bad.push("A".repeat(64)); // uppercase
        bad.push("g".repeat(64)); // not hex
        bad.push("a".repeat(63));
        bad.push("a".repeat(65));
        bad.push(String::new());
        bad.push(format!("{}{}", "a".repeat(62), "+1"));
        for value in bad {
            let wire = format!("\"{value}\"");
            let error = serde_json::from_str::<BindingCredential>(&wire).unwrap_err();
            assert!(
                value.is_empty() || !error.to_string().contains(&value),
                "error echoes submitted credential: {error}"
            );
        }
        assert!(serde_json::from_str::<BindingCredential>("123").is_err());
    }

    #[test]
    fn secrets_are_redacted_in_debug_and_errors() {
        let credential = credential();
        let hex = serde_json::to_string(&credential)
            .unwrap()
            .trim_matches('"')
            .to_string();
        assert!(!format!("{credential:?}").contains(&hex));
        assert!(!format!("{:?}", hello()).contains(&hex));
        for reject in [
            Reject::UnsupportedVersion,
            Reject::InvalidCredential,
            Reject::AlreadyConnected,
            Reject::Revoked,
        ] {
            assert!(!reject.to_string().contains(&hex));
        }
    }

    #[test]
    fn hello_roundtrips_as_single_wire_dto() {
        let hello = hello();
        let json = serde_json::to_value(&hello).unwrap();
        assert_eq!(json["generation"], serde_json::json!("1"));
        assert_eq!(
            json["capabilities"],
            serde_json::json!(["files", "process"])
        );
        let decoded: Hello = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(decoded.generation, hello.generation);
        assert_eq!(decoded.workspace, hello.workspace);

        let mut wrong_type = json;
        wrong_type["generation"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Hello>(wrong_type).is_err());
    }

    #[test]
    fn workspace_predicate_is_lexical_and_exact() {
        for good in ["/", "/work", "/work/root", "/a-b_c.d/e"] {
            assert!(is_normalized_absolute_path(Path::new(good)), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            "work",
            "work/root",
            "./work",
            "/work/",
            "/work//root",
            "/work/./root",
            "/work/../root",
            "/..",
        ] {
            assert!(!is_normalized_absolute_path(Path::new(bad)), "{bad}");
        }
    }
}
