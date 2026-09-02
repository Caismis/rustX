//! The local runtime process lifecycle over the Issue #38 stdio/JSONL
//! transport.
//!
//! # Output contract
//!
//! ```text
//! before serving : stdout is exactly empty
//! while serving  : stdout is Runtime Client JSONL records only
//! diagnostics    : stderr, always
//! ```
//!
//! Startup configuration failure writes a bounded diagnostic to stderr,
//! exits non-zero, and leaves stdout at **zero bytes** — composition
//! finishes entirely before the transport is created, so no partial
//! protocol frame can exist.
//!
//! # Exit semantics
//!
//! - clean input EOF at a record boundary, or a peer broken pipe, ends this
//!   one-active-lineage process **successfully**;
//! - malformed framing or any other transport error writes a diagnostic to
//!   stderr and exits **non-zero**;
//! - semantic `shutdown` responds only after the conversation runtime reaches
//!   quiescence, and does **not** close the transport. A controlling client
//!   closes the transport or the process according to its own lifecycle
//!   policy.
//!
//! Transport EOF remains a detach, never an Agent Loop cancellation
//! primitive, and this module delegates semantic M9 recovery and runtime
//! quiescence to the conversation runtime rather than implementing either
//! concern in the transport process.

use std::io::Write;

use crate::runtime_client::transport::stdio::{StdioSessionEnd, serve_stdio_jsonl};

use super::cli::{USAGE, parse_arguments};
use super::composition::{
    LocalConversationInspection, LocalRuntimeDependencies, LocalSessionProduct, StartupSession,
};

/// The deterministic terminal outcome of the local runtime process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessOutcome {
    /// The transport closed cleanly; the process exits with code 0.
    TransportClosed(StdioSessionEnd),
    /// Startup configuration failed; nothing was ever written to stdout.
    StartupFailed(String),
    /// The transport terminated abnormally after serving began.
    TransportFailed(String),
}

impl ProcessOutcome {
    /// The process exit code of this outcome.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::TransportClosed(_) => 0,
            Self::StartupFailed(_) => 2,
            Self::TransportFailed(_) => 1,
        }
    }

    /// The bounded stderr diagnostic of this outcome, when it has one.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::TransportClosed(_) => None,
            Self::StartupFailed(detail) | Self::TransportFailed(detail) => Some(detail),
        }
    }
}

enum ServingRuntime {
    Session(Box<LocalSessionProduct>),
    Inspection(LocalConversationInspection),
}

/// Composes the runtime from explicit arguments and serves it on
/// stdin/stdout.
///
/// Returns the terminal outcome instead of exiting, so the binary owns the
/// single exit point and tests can drive the same code path.
pub async fn serve(arguments: impl IntoIterator<Item = String>) -> ProcessOutcome {
    let paths = match parse_arguments(arguments) {
        Ok(paths) => paths,
        Err(error) => return ProcessOutcome::StartupFailed(format!("{error}\n{USAGE}")),
    };
    // Composition completes — including the initial capability commit —
    // before the transport exists, so a startup failure can never leave a
    // partially initialized protocol server.
    let runtime = match &paths.startup_session {
        StartupSession::InspectConversation { conversation_id } => {
            match LocalConversationInspection::compose(&paths, conversation_id) {
                Ok(runtime) => ServingRuntime::Inspection(runtime),
                Err(error) => return ProcessOutcome::StartupFailed(error.to_string()),
            }
        }
        _ => match LocalSessionProduct::compose(&paths, &LocalRuntimeDependencies::default()).await
        {
            Ok(runtime) => ServingRuntime::Session(Box::new(runtime)),
            Err(error) => return ProcessOutcome::StartupFailed(error.to_string()),
        },
    };
    let endpoint = match &runtime {
        ServingRuntime::Session(runtime) => runtime.endpoint(),
        ServingRuntime::Inspection(runtime) => runtime.endpoint(),
    };
    match serve_stdio_jsonl(endpoint).await {
        Ok(end) => ProcessOutcome::TransportClosed(end),
        Err(error) => ProcessOutcome::TransportFailed(error.to_string()),
    }
}

/// Runs the process to its single exit point.
///
/// Diagnostics go to stderr with `writeln!`; `println!` is never used. In
/// the normal mode stdout carries protocol records and nothing else; in
/// the internal `--subagent-child` mode stdout is instead owned by the
/// Activity observation IPC (Issue #178).
pub async fn run_process(arguments: impl IntoIterator<Item = String>) -> i32 {
    let arguments: Vec<String> = arguments.into_iter().collect();
    // The internal subagent-child mode (Issue #60): one exact flag, no
    // paths — the typed startup specification arrives over the inherited
    // control channel (fd 0).
    if arguments
        .iter()
        .any(|argument| argument == "--subagent-child")
    {
        if arguments.len() != 1 {
            let mut stderr = std::io::stderr();
            let _ = writeln!(
                stderr,
                "rustx: --subagent-child is an internal mode and takes no other arguments"
            );
            let _ = stderr.flush();
            return 2;
        }
        return Box::pin(super::subagent_child::run_subagent_child()).await;
    }
    let outcome = serve(arguments).await;
    if let Some(diagnostic) = outcome.diagnostic() {
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "rustx: {diagnostic}");
        let _ = stderr.flush();
    }
    outcome.exit_code()
}
