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

/// A stdio argument whose path is resolved by the Host and checked by the Client.
#[derive(Debug, PartialEq, Eq)]
pub struct StdioArgumentPath<'a> {
    /// Index of the complete argument in argv.
    pub index: usize,
    /// Text before the path, including any option separator.
    pub prefix: &'a str,
    /// Path spelling supplied by the caller.
    pub value: &'a str,
    /// How the Client checks the resolved path.
    pub kind: StdioPathKind,
}

/// Physical meaning of a stdio path on the Execution Client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioPathKind {
    /// A file or directory that must already exist.
    Input,
    /// An existing directory, as required by `env --chdir`.
    Directory,
    /// A path-qualified Node preload that may use `CommonJS` extension lookup.
    CommonJs,
    /// An explicit path that may name a new leaf in a writable directory.
    WritableLeaf,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operand {
    Value,
    Input,
    Directory,
    Loader,
    CommonJs,
    Opaque,
    Inline,
}

fn interpreter_operand(name: &str, option: &str) -> Option<Operand> {
    let shell = matches!(name, "sh" | "bash" | "dash" | "zsh");
    if (shell
        && option.strip_prefix('-').is_some_and(|flags| {
            flags.bytes().all(|byte| byte.is_ascii_alphabetic()) && flags.contains('c')
        }))
        || (matches!(name, "node" | "nodejs")
            && matches!(option, "-e" | "--eval" | "-p" | "--print"))
        || (name.starts_with("python") && matches!(option, "-c" | "-m"))
    {
        Some(Operand::Inline)
    } else if (shell && matches!(option, "-o" | "+o" | "-O" | "+O"))
        || (name.starts_with("python") && matches!(option, "-W" | "-X"))
    {
        Some(Operand::Opaque)
    } else if matches!(name, "node" | "nodejs") {
        match option {
            "-r" | "--require" => Some(Operand::CommonJs),
            "--import" | "--loader" | "--experimental-loader" => Some(Operand::Loader),
            _ => None,
        }
    } else {
        None
    }
}

fn stdio_input_option(name: &str) -> bool {
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

fn explicit_stdio_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || matches!(value, "." | "..")
        || foreign_stdio_path(value)
}

fn argument_path<'a>(
    index: usize,
    prefix: &'a str,
    value: &'a str,
    role: Operand,
) -> Option<StdioArgumentPath<'a>> {
    if value.is_empty() || value == "-" || value.contains("://") {
        return None;
    }
    let kind = match role {
        Operand::Input => StdioPathKind::Input,
        Operand::Directory => StdioPathKind::Directory,
        Operand::Loader if explicit_stdio_path(value) => StdioPathKind::Input,
        Operand::CommonJs if explicit_stdio_path(value) => StdioPathKind::CommonJs,
        Operand::Value if explicit_stdio_path(value) => StdioPathKind::WritableLeaf,
        _ => return None,
    };
    Some(StdioArgumentPath {
        index,
        prefix,
        value,
        kind,
    })
}

fn split_option(arg: &str) -> (&str, &str, Option<&str>) {
    arg.split_once('=')
        .map_or((arg, "", None), |(flag, value)| {
            (flag, &arg[..=flag.len()], Some(value))
        })
}

fn env_option(arg: &str) -> (&str, &str, Option<&str>) {
    if let Some(short) = arg
        .strip_prefix('-')
        .filter(|flags| !flags.starts_with('-'))
    {
        let rest = short.trim_start_matches(['i', 'v', '0']);
        let flag = match rest.as_bytes().first() {
            Some(b'a') => "-a",
            Some(b'C') => "-C",
            Some(b'S') => "-S",
            Some(b'u') => "-u",
            _ => return (arg, "", None),
        };
        let suffix = &rest[1..];
        return if suffix.is_empty() {
            (flag, "", None)
        } else {
            (flag, &arg[..arg.len() - suffix.len()], Some(suffix))
        };
    }
    split_option(arg)
}

fn env_paths(args: &[String]) -> (usize, Option<&Path>, Vec<StdioArgumentPath<'_>>) {
    let mut args_with_index = args.iter().enumerate();
    let mut paths = Vec::new();
    let mut options = true;
    while let Some((index, arg)) = args_with_index.next() {
        if options && arg == "--" {
            options = false;
        } else if options && arg.starts_with('-') && arg != "-" {
            let (flag, prefix, attached) = env_option(arg);
            let role = match flag {
                "-C" | "--chdir" => Operand::Directory,
                "--env0-from" => Operand::Input,
                "-a" | "-u" | "--argv0" | "--unset" | "--quoting-style" => Operand::Opaque,
                // Leave env's expansion opaque and classify the remaining
                // explicit arguments without guessing the interpreter.
                "-S" | "--split-string" => {
                    if attached.is_none() {
                        args_with_index.next();
                    }
                    return (
                        args_with_index
                            .next()
                            .map_or(args.len(), |(index, _)| index),
                        None,
                        paths,
                    );
                }
                _ => continue,
            };
            if let Some(value) = attached {
                paths.extend(argument_path(index, prefix, value, role));
            } else if let Some((value_index, value)) = args_with_index.next() {
                paths.extend(argument_path(value_index, "", value, role));
            }
        } else if arg.contains('=') || options && arg == "-" {
            options = false;
        } else {
            let role = if arg.contains('/') {
                Operand::Input
            } else {
                Operand::Value
            };
            paths.extend(argument_path(index, "", arg, role));
            return (index + 1, Some(Path::new(arg)), paths);
        }
    }
    (args.len(), None, paths)
}

/// Walk argv using the same path roles before and after Host resolution.
/// Ambiguous arguments need an explicit absolute or dot-relative path.
pub fn stdio_argument_paths<'a>(
    program: &'a Path,
    args: &'a [String],
) -> impl Iterator<Item = StdioArgumentPath<'a>> {
    let (start, interpreter, wrapper_paths) =
        if program.file_name().is_some_and(|name| name == "env") {
            env_paths(args)
        } else {
            (0, Some(program), Vec::new())
        };
    let name = interpreter
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let shell = matches!(name, "sh" | "bash" | "dash" | "zsh");
    let mut before_script =
        shell || matches!(name, "node" | "nodejs") || name.starts_with("python");
    let mut options = true;
    let mut next = None;
    wrapper_paths
        .into_iter()
        .chain(
            args.iter()
                .enumerate()
                .skip(start)
                .filter_map(move |(index, arg)| {
                    if let Some(role) = next.take() {
                        return argument_path(index, "", arg, role);
                    }
                    if options && arg == "--" {
                        options = false;
                        return None;
                    }
                    if options
                        && (arg.starts_with('-') && arg != "-"
                            || before_script && shell && arg.starts_with('+'))
                    {
                        let (flag, prefix, value) = split_option(arg);
                        let role = if before_script {
                            interpreter_operand(name, flag)
                        } else {
                            None
                        }
                        .or_else(|| stdio_input_option(flag).then_some(Operand::Input));
                        if role == Some(Operand::Inline) {
                            before_script = false;
                        }
                        if let Some(value) = value {
                            return argument_path(
                                index,
                                prefix,
                                value,
                                role.unwrap_or(Operand::Value),
                            );
                        }
                        next = role;
                        return None;
                    }
                    let role = if std::mem::take(&mut before_script) {
                        // Only the interpreter consumes its own separator. The script
                        // receives a fresh argv and can have file options of its own.
                        options = true;
                        Operand::Input
                    } else {
                        Operand::Value
                    };
                    argument_path(index, "", arg, role)
                }),
        )
}

#[cfg(test)]
mod stdio_path_tests {
    use super::*;
    use StdioPathKind::{CommonJs, Directory, Input, WritableLeaf};

    const NO_PATHS: [(usize, StdioPathKind); 0] = [];

    fn path_values(program: &str, args: &[&str]) -> Vec<(usize, StdioPathKind)> {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        stdio_argument_paths(Path::new(program), &args)
            .map(|arg| (arg.index, arg.kind))
            .collect()
    }

    #[test]
    fn node_preserves_bare_specifiers_and_literal_values() {
        assert_eq!(
            path_values(
                "/usr/bin/env",
                &[
                    "node",
                    "--require",
                    "dotenv/config",
                    "--require=./preload",
                    "--import=@scope/package",
                    "--loader=./loader.mjs",
                    "server.js",
                    "--model=gpt-4.1-mini",
                    "owner/repo",
                    "report.json",
                    "--config=settings",
                    "--log=./mcp.log",
                ],
            ),
            [
                (3, CommonJs),
                (5, Input),
                (6, Input),
                (10, Input),
                (11, WritableLeaf)
            ]
        );
    }

    #[test]
    fn wrapper_values_and_interpreter_separators_keep_their_roles() {
        assert_eq!(
            path_values(
                "env",
                &[
                    "--unset",
                    "PYTHONPATH",
                    "--chdir=runtime",
                    "APP=foo/bar",
                    "python3",
                    "--",
                    "server.py",
                    "--config=settings.toml",
                    "--input",
                    "requests.json",
                    "--",
                    "--config=literal",
                ],
            ),
            [(2, Directory), (6, Input), (7, Input), (9, Input)]
        );
        assert_eq!(
            path_values(
                "env",
                &["-iC./runtime", "-u", "NODE_PATH", "node", "server.js"]
            ),
            [(0, Directory), (4, Input)]
        );
        assert_eq!(
            path_values("env", &["-C", "runtime", "--", "node", "server.js"]),
            [(1, Directory), (4, Input)]
        );
    }

    #[test]
    fn interpreter_payloads_and_urls_remain_opaque() {
        for flag in ["-c", "-lc", "-ic"] {
            assert_eq!(path_values("bash", &[flag, "/some/program"]), NO_PATHS);
        }
        for flag in ["-O", "+O", "-o"] {
            assert_eq!(
                path_values("bash", &[flag, "extglob", "./server.sh"]),
                [(2, Input)]
            );
        }
        assert_eq!(
            path_values("bash", &["--rcfile", "startup", "./server.sh"]),
            [(1, Input), (2, Input)]
        );
        assert_eq!(
            path_values("python3", &["-W", "ignore", "server.py"]),
            [(2, Input)]
        );
        assert_eq!(
            path_values("/usr/bin/env", &["python3", "-m", "package.server"]),
            NO_PATHS
        );
        assert_eq!(path_values("node", &["--eval=/program/text"]), NO_PATHS);
        assert_eq!(
            path_values("server", &["--input=https://example.test/feed"]),
            NO_PATHS
        );
    }

    #[test]
    fn env_split_strings_preserve_explicit_paths_in_following_arguments() {
        assert_eq!(
            path_values("env", &["-S", "python3 -u ./opaque.py", "./server.py"]),
            [(2, WritableLeaf)]
        );
        assert_eq!(
            path_values(
                "env",
                &["--split-string=python3 -u ./opaque.py", "./server.py"]
            ),
            [(1, WritableLeaf)]
        );
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
