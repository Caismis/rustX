//! The `rustx` process entry: one routing point before any effect.
//!
//! ```text
//! rustx app-server --listen stdio|ws://IP:PORT  -> the App Server (the only product control protocol)
//! rustx config|doctor|workflow|init ...         -> native offline configuration owners
//! rustx --subagent-child                        -> internal typed child startup (fd 0 control IPC)
//! rustx | rustx --help | rustx help ...         -> help on stderr, exit 0
//! anything else                                 -> bounded usage diagnostic on stderr, exit 2
//! ```
//!
//! Parsing completes before host capture, configuration reads, storage, or
//! any runtime composition, so help and rejected invocations have no effect
//! and write nothing to stdout. Configuration subcommands own stdout for their
//! bounded human/JSON reports; App Server stdout belongs to its protocol.

use std::io::Write;

use std::ffi::{OsStr, OsString};

/// Runs the process to its single exit point.
///
/// Help and argument diagnostics go to stderr; configuration commands write
/// bounded human/JSON results to stdout. App Server stdout carries protocol
/// records only; in the internal `--subagent-child` mode stdout is instead
/// owned by the Activity observation IPC (Issue #178).
pub async fn run_process(arguments: impl IntoIterator<Item = impl Into<OsString> + Clone>) -> i32 {
    let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
    // The internal subagent-child mode (Issue #60): one exact flag, no
    // paths — the typed startup specification arrives over the inherited
    // control channel (fd 0).
    if arguments
        .iter()
        .any(|argument| argument == OsStr::new("--subagent-child"))
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
    let command = match super::cli::parse_command(arguments) {
        Ok(command) => command,
        Err(error) => {
            return if writeln!(std::io::stderr(), "{error}").is_ok() {
                2
            } else {
                1
            };
        }
    };
    match command {
        super::cli::Command::Help(help) => i32::from(write!(std::io::stderr(), "{help}").is_err()),
        super::cli::Command::AppServer(request) => {
            crate::app_server::process::run_process(request).await
        }
        command => Box::pin(run_configuration_command(command)).await,
    }
}

#[allow(clippy::too_many_lines)] // finite command routing and output ownership
async fn run_configuration_command(command: super::cli::Command) -> i32 {
    use super::cli::Command;
    use super::diagnostics::{Report, Validity};
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
        Command::Init { request, json } => {
            let report = match super::initialization::documents(&request) {
                Ok(documents) => {
                    let result = super::initialization::initialize(&host, &documents);
                    let mut report = Report::new("init");
                    if result.failed.is_some() || !result.conflicts.is_empty() {
                        report.validity = Validity::Invalid;
                    }
                    report.initialization = Some(result);
                    report
                }
                Err(reason) => Report::failure(
                    "init",
                    None,
                    "arguments",
                    &reason,
                    "supply explicit declarations shown by rustx init --help",
                ),
            };
            (report, json)
        }
        Command::Workflow {
            id,
            explain,
            request,
            json,
        } => (
            super::workflow_inspection::inspect(&id, explain, &request, &host),
            json,
        ),
        Command::Check { request, json } => (
            super::diagnostics::inspect("config_check", &request, &host).0,
            json,
        ),
        Command::Show {
            request,
            json,
            agent,
        } => {
            let (mut report, launch) = super::diagnostics::inspect("config_show", &request, &host);
            if let (Some(name), Some(launch)) = (agent, launch) {
                let selected = if name == "main" {
                    launch.inspection.main.as_ref()
                } else {
                    crate::runtime::subagent::SubagentName::parse(&name)
                        .ok()
                        .and_then(|name| launch.inspection.agents.get(&name))
                };
                if let Some(selected) = selected {
                    report.agent = Some(selected.clone());
                    report.launch = None;
                    report.capabilities = None;
                } else {
                    report = super::diagnostics::Report::failure(
                        "config_show",
                        None,
                        "agent",
                        "Agent not discovered",
                        "select main or a discovered named Agent",
                    );
                }
            }
            (report, json)
        }
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
                } else if report.validity == Validity::Invalid {
                    2
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
        Command::Help(_) | Command::AppServer(_) => unreachable!(),
    };
    let code = report.exit_code();
    if writeln!(std::io::stdout(), "{}", report.render(json)).is_err() {
        return 1;
    }
    code
}
