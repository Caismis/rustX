//! The local runtime process lifecycle over the Issue #38 stdio/JSONL
//! transport.
//!
//! # Output contract
//!
//! ```text
//! before serving : stdout is exactly empty
//! while serving  : stdout is Runtime Client JSONL records only
//! diagnostics    : stderr for runtime transport startup
//! ```
//! Configuration subcommands own stdout for their human/JSON reports; they do
//! not enter the Runtime Client transport or create a Session.
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
    /// A host-owned trust operation completed without runtime composition.
    TrustChanged,
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
            Self::TransportClosed(_) | Self::TrustChanged => 0,
            Self::StartupFailed(_) => 2,
            Self::TransportFailed(_) => 1,
        }
    }

    /// The bounded stderr diagnostic of this outcome, when it has one.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::TransportClosed(_) | Self::TrustChanged => None,
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
    let request = match parse_arguments(arguments) {
        Ok(paths) => paths,
        Err(error) => return ProcessOutcome::StartupFailed(format!("{error}\n{USAGE}")),
    };
    Box::pin(serve_request(request)).await
}

async fn serve_request(request: super::launch::LaunchRequest) -> ProcessOutcome {
    let host = match super::launch::HostEnvironment::capture() {
        Ok(host) => host,
        Err(error) => return ProcessOutcome::StartupFailed(error),
    };
    if let Some(action) = request.trust {
        return match super::launch::change_trust(&request, &host, action) {
            Ok(()) => ProcessOutcome::TrustChanged,
            Err(error) => ProcessOutcome::StartupFailed(error),
        };
    }
    // Composition completes — including the initial capability commit —
    // before the transport exists, so a startup failure can never leave a
    // partially initialized protocol server.
    let runtime =
        if let StartupSession::InspectConversation { conversation_id } = &request.startup_session {
            let locations = match super::launch::resolve_inspection_locations(&request, &host) {
                Ok(locations) => locations,
                Err(error) => return ProcessOutcome::StartupFailed(error),
            };
            match LocalConversationInspection::compose(&locations, conversation_id).await {
                Ok(runtime) => ServingRuntime::Inspection(runtime),
                Err(error) => return ProcessOutcome::StartupFailed(error.to_string()),
            }
        } else {
            let paths = match super::launch::resolve(&request, &host) {
                Ok(paths) => paths,
                Err(error) => return ProcessOutcome::StartupFailed(error),
            };
            match LocalSessionProduct::compose(&paths, &LocalRuntimeDependencies::default()).await {
                Ok(runtime) => ServingRuntime::Session(Box::new(runtime)),
                Err(error) => return ProcessOutcome::StartupFailed(error.to_string()),
            }
        };
    let served = match runtime {
        ServingRuntime::Session(runtime) => serve_stdio_jsonl(runtime.endpoint()).await,
        ServingRuntime::Inspection(runtime) => runtime.serve().await,
    };
    match served {
        Ok(end) => ProcessOutcome::TransportClosed(end),
        Err(error) => ProcessOutcome::TransportFailed(error.to_string()),
    }
}

/// Runs the process to its single exit point.
///
/// Runtime transport diagnostics go to stderr; configuration commands instead
/// write bounded human/JSON results to stdout. In
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
    let json_error = super::cli::diagnostic_json_requested(&arguments);
    let Ok(command) = super::cli::parse_command(arguments) else {
        if json_error {
            let report = super::diagnostics::Report::failure(
                "command",
                None,
                "arguments",
                "invalid command arguments",
                "use the finite grammar shown by rustx --help",
            );
            return if writeln!(std::io::stdout(), "{}", report.render(true)).is_ok() {
                2
            } else {
                1
            };
        }
        let _ = writeln!(
            std::io::stderr(),
            "rustx: invalid command arguments\n{USAGE}\n{}",
            super::cli::CONFIG_USAGE
        );
        return 2;
    };
    let request = match command {
        super::cli::Command::Launch(request) => request,
        command => return Box::pin(run_configuration_command(command)).await,
    };
    let outcome = Box::pin(serve_request(request)).await;
    if let Some(diagnostic) = outcome.diagnostic() {
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "rustx: {diagnostic}");
        let _ = stderr.flush();
    }
    outcome.exit_code()
}

#[allow(clippy::too_many_lines)] // finite command routing and output ownership
async fn run_configuration_command(command: super::cli::Command) -> i32 {
    use super::cli::Command;
    use super::diagnostics::{Report, Validity};
    if matches!(command, Command::Help) {
        let _ = writeln!(std::io::stdout(), "{USAGE}\n{}", super::cli::CONFIG_USAGE);
        return 0;
    }
    let Ok(host) = super::launch::HostEnvironment::capture() else {
        let report = Report::failure(
            "command",
            None,
            "host.paths",
            "cannot discover absolute host paths",
            "set absolute HOME/XDG paths and use an existing launch directory",
        );
        return if writeln!(
            std::io::stdout(),
            "{}",
            report.render(command.json_output())
        )
        .is_ok()
        {
            2
        } else {
            1
        };
    };
    let (report, json) = match command {
        Command::Init { arguments, json } => {
            let report = match super::initialization::documents(&arguments) {
                Ok(documents) => {
                    let result = super::initialization::initialize(&host, &documents);
                    let mut report = Report::new("init");
                    if result.failed.is_some() || !result.conflicts.is_empty() {
                        report.validity = Validity::Invalid;
                    }
                    report.initialization = Some(result);
                    report
                }
                Err(reason) => {
                    let mut report = Report::failure(
                        "init",
                        None,
                        "arguments",
                        &reason,
                        "supply explicit declarations shown by rustx --help",
                    );
                    if arguments.is_empty() {
                        report.validity = Validity::Incomplete;
                    }
                    report
                }
            };
            (report, json)
        }
        Command::Check { request, json } => (
            super::diagnostics::inspect("config_check", &request, &host).0,
            json,
        ),
        Command::Show { request, json } => (
            super::diagnostics::inspect("config_show", &request, &host).0,
            json,
        ),
        Command::Doctor {
            request,
            json,
            prepare,
        } => {
            let (report, launch) = super::diagnostics::inspect("doctor", &request, &host);
            if let Some(launch) = launch {
                // Install cancellation listeners before publishing a plan or starting effects.
                let (Ok(mut interrupt), Ok(mut terminate)) = (
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()),
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()),
                ) else {
                    return 1;
                };
                let plan = super::probes::plan(&launch, prepare);
                // Flush effect disclosure before the first credential or external boundary.
                let mut stdout = std::io::stdout();
                if writeln!(stdout, "{}", plan.render(json))
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    return 1;
                }
                let cancellation = crate::runtime::CancellationSignal::new();
                let execution = super::probes::execute(&launch, &plan, cancellation.clone());
                tokio::pin!(execution);
                let results = tokio::select! {
                    results = &mut execution => results,
                    _ = interrupt.recv() => { cancellation.cancel(); execution.await },
                    _ = terminate.recv() => { cancellation.cancel(); execution.await },
                };
                let code = if results.iter().any(|result| {
                    matches!(
                        result.state,
                        super::probes::ProbeState::Failed
                            | super::probes::ProbeState::TimedOut
                            | super::probes::ProbeState::Cancelled
                    )
                }) {
                    1
                } else {
                    3
                };
                if writeln!(stdout, "{}", super::probes::render_results(&results, json))
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    return 1;
                }
                return code;
            }
            (report, json)
        }
        Command::Help | Command::Launch(_) => unreachable!(),
    };
    let code = report.exit_code();
    if writeln!(std::io::stdout(), "{}", report.render(json)).is_err() {
        return 1;
    }
    code
}
