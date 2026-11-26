//! Execution transport for an admitted Client-owned stdio MCP session.
//! The application peer's protocol is distinct from the `RustX` binding.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{DecimalU64, process::ProcessOperationKey};

/// Reject Windows drive and UNC syntax in this Unix Client protocol.
#[must_use]
pub fn foreign_stdio_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || value.starts_with("\\\\")
}

/// Recognize path-shaped stdio operands without treating URLs or package names as files.
/// The optional prefix is the option name of a `--option=PATH` argument.
#[must_use]
pub fn stdio_path_operand(arg: &str, required: bool) -> Option<(Option<&str>, &str)> {
    let (prefix, value) = if arg.starts_with('-') {
        let (name, value) = arg.split_once('=')?;
        (Some(name), value)
    } else {
        (None, arg)
    };
    let path = Path::new(value);
    let filename_like = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.bytes().any(|byte| byte.is_ascii_alphabetic()));
    (!value.contains("://")
        && !value.starts_with('@')
        && (required
            || prefix.is_some_and(stdio_input_option)
            || value.contains('/')
            || value.starts_with('.')
            || filename_like
            || foreign_stdio_path(value)))
    .then_some((prefix, value))
}

/// Whether this interpreter flag is followed by program text or a module name.
#[must_use]
pub fn stdio_inline_script(program: &Path, argument: &str) -> bool {
    let executable = program
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let shell = matches!(executable, "sh" | "bash" | "dash" | "zsh");
    let shell_command = shell
        && argument.strip_prefix('-').is_some_and(|options| {
            !options.is_empty()
                && options.bytes().all(|byte| byte.is_ascii_alphabetic())
                && options.contains('c')
        });
    shell_command
        || ((matches!(executable, "node" | "nodejs") || executable.starts_with("python"))
            && matches!(argument, "-c" | "-e" | "--eval"))
        || (executable.starts_with("python") && argument == "-m")
}

/// Find an interpreter explicitly launched through `env` without executing it.
#[must_use]
pub fn stdio_interpreter<'a>(program: &'a Path, args: &'a [String]) -> &'a Path {
    if program.file_name().is_some_and(|name| name == "env") {
        args.iter()
            .find(|arg| !arg.starts_with('-') && !arg.contains('='))
            .map_or(program, |arg| Path::new(arg))
    } else {
        program
    }
}

/// Index of an interpreter's positional script, excluding inline code and module names.
#[must_use]
pub fn stdio_script_index(program: &Path, args: &[String]) -> Option<usize> {
    let interpreter = stdio_interpreter(program, args);
    let name = interpreter.file_name()?.to_str()?;
    if !matches!(name, "sh" | "bash" | "dash" | "zsh" | "node" | "nodejs")
        && !name.starts_with("python")
    {
        return None;
    }
    let start = if program.file_name().is_some_and(|name| name == "env") {
        args.iter()
            .position(|arg| !arg.starts_with('-') && !arg.contains('='))?
            + 1
    } else {
        0
    };
    let shell = matches!(name, "sh" | "bash" | "dash" | "zsh");
    let mut option_value = false;
    for (index, argument) in args.iter().enumerate().skip(start) {
        if option_value {
            option_value = false;
            continue;
        }
        if stdio_inline_script(interpreter, argument) {
            return None;
        }
        if (name.starts_with("python") && matches!(argument.as_str(), "-W" | "-X"))
            || (shell
                && matches!(
                    argument.as_str(),
                    "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file"
                ))
            || (matches!(name, "node" | "nodejs")
                && matches!(
                    argument.as_str(),
                    "-r" | "--require" | "--import" | "--loader"
                ))
        {
            option_value = true;
            continue;
        }
        if argument == "--" {
            return (index + 1 < args.len()).then_some(index + 1);
        }
        if !(argument.starts_with('-') || shell && argument.starts_with('+')) {
            return Some(index);
        }
    }
    None
}

/// Common options that explicitly identify a pre-existing input file.
#[must_use]
pub fn stdio_input_option(name: &str) -> bool {
    matches!(
        name,
        "--config"
            | "--config-file"
            | "--input"
            | "--input-file"
            | "--script"
            | "--rcfile"
            | "--init-file"
    )
}

#[cfg(test)]
mod stdio_path_tests {
    use super::*;

    #[test]
    fn input_urls_and_shell_option_values_remain_arguments() {
        for option in ["-O", "+O", "-o", "--rcfile"] {
            let args = [option, "extglob", "./server.sh"].map(str::to_owned);
            assert_eq!(stdio_script_index(Path::new("bash"), &args), Some(2));
        }
        assert_eq!(stdio_path_operand("https://example.test/feed", true), None);
        assert_eq!(
            stdio_path_operand("--input=https://example.test/feed", false),
            None
        );
        assert_eq!(
            stdio_path_operand("--config=settings", false),
            Some((Some("--config"), "settings"))
        );
    }

    #[test]
    fn combined_shell_flags_and_interpreter_options_keep_their_meaning() {
        for flag in ["-c", "-lc", "-ic"] {
            assert!(stdio_inline_script(Path::new("bash"), flag));
            let args = vec![flag.into(), "printf 'x/y'".into()];
            assert_eq!(stdio_script_index(Path::new("bash"), &args), None);
        }
        let args = vec!["-W".into(), "ignore".into(), "server.py".into()];
        assert_eq!(stdio_script_index(Path::new("python3"), &args), Some(2));
        let args = vec!["python3".into(), "-m".into(), "package.server".into()];
        assert_eq!(stdio_script_index(Path::new("/usr/bin/env"), &args), None);
    }
}

/// Maximum demanded Client-owned sources in one capability generation.
pub const MAX_GENERATION_SESSIONS: usize = 8;
/// Physical occupancy includes a complete candidate and its retained predecessor.
pub const MAX_SESSIONS: usize = 2 * MAX_GENERATION_SESSIONS;
/// Maximum negotiated or inbound MCP record before splitting it into pages.
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
/// A complete outbound record fits in one Execution frame after base64.
pub const MAX_SEND_BYTES: usize = 512 * 1024;
/// Maximum complete UTF-8 startup stderr retained from one settled process.
pub const MAX_STARTUP_STDERR_BYTES: usize = 64 * 1024;
/// Pending outbound records per session.
pub const MAX_PENDING_SENDS: usize = 4;

/// Maximum package directories and encoded path bytes in a capture receipt.
pub const MAX_SOURCE_WATCH_DIRECTORIES: usize = 128;
pub const MAX_SOURCE_WATCH_BYTES: usize = 16 * 1024;

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
        /// Package-relative directories, including the empty package root.
        /// Absence means native watch coverage needs periodic recapture.
        watch_directories: Option<Vec<PathBuf>>,
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
                watch_directories,
            } => formatter
                .debug_struct("Captured")
                .field("digest", digest)
                .field("has_permissions", &permissions_base64.is_some())
                .field(
                    "watch_directories",
                    &watch_directories.as_ref().map(Vec::len),
                )
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
        /// Canonical source identity for this frozen grant.
        source_id: String,
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

/// Evidence from the startup process stderr pipe after physical settlement.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum StartupStderr {
    /// Complete UTF-8 from a pipe that reached EOF within the capture limit.
    Complete {
        /// Untrusted text. Redact with the admitted source's credentials.
        text: String,
    },
    /// Clipped, invalid UTF-8, unavailable or not yet complete at the cutoff.
    Incomplete,
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
    /// Startup failed and the physical owner has settled.
    StartupFailed {
        /// The original startup failure classification.
        failure: Failure,
        /// Bounded evidence from that exact process.
        stderr: StartupStderr,
    },
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
            Self::StartupFailed { failure, .. } => formatter
                .debug_struct("StartupFailed")
                .field("failure", failure)
                .finish_non_exhaustive(),
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
