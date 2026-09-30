//! Public lexical grammar. Native owners resolve intent and perform effects.
use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum, builder::NonEmptyStringValueParser};

use super::initialization::{InitializationRequest, Template};
use super::launch::LaunchRequest;

/// Explicit native command intent; no clap types cross this boundary.
#[derive(Debug)]
pub enum Command {
    Workflow {
        id: crate::runtime::workflow::WorkflowId,
        explain: bool,
        request: LaunchRequest,
        json: bool,
    },
    Help(String),
    Check {
        request: LaunchRequest,
        json: bool,
    },
    Show {
        agent: Option<String>,
        request: LaunchRequest,
        json: bool,
    },
    Doctor {
        request: LaunchRequest,
        json: bool,
        prepare: bool,
    },
    Init {
        request: InitializationRequest,
        json: bool,
    },
    AppServer(crate::app_server::process::Request),
}

impl Command {
    pub(super) const fn json_output(&self) -> bool {
        match self {
            Self::Workflow { json, .. }
            | Self::Check { json, .. }
            | Self::Show { json, .. }
            | Self::Doctor { json, .. }
            | Self::Init { json, .. } => *json,
            Self::Help(_) | Self::AppServer(_) => false,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "rustx",
    about = "The rustX App Server and offline configuration tools",
    arg_required_else_help = true,
    after_help = "Clients control rustX only through `rustx app-server`; a bare invocation prints this help.\nOmitted selections are resolved by native rustX policy. Static inspection never prepares execution.\nExit: 0 complete/help; 1 probe or output failure; 2 invalid; 3 incomplete or unresolved readiness.\nHelp and lexical errors use stderr; --json applies only to parsed diagnostic commands."
)]
struct Cli {
    #[command(subcommand)]
    command: PublicCommand,
}

#[derive(Debug, Subcommand)]
enum PublicCommand {
    /// Inspect discovered workflows offline
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Inspect prospective configuration without execution
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Disclose a probe plan, then perform explicitly permitted probes
    Doctor(DoctorArgs),
    /// Publish minimal user configuration without overwriting existing files
    #[command(
        after_help = "Native template policy: custom uses a complete --model-document. Other templates require model-id, context-window, max-output, tool-calls and reasoning declarations. OpenAI templates additionally require explicit compatibility TOML; no capabilities or compatibility are inferred. Credential-env names an environment variable, never a literal secret. Publication creates ~/rustx/rustx.toml and ~/rustx/.agents without overwriting existing files."
    )]
    Init(InitArgs),
    /// Serve the App Server protocol on an explicitly selected transport
    #[command(
        after_help = "Native bindings default to ~/rustx/rustx.toml and ~/rustx/runtime only when omitted. User resources remain ~/rustx/.agents. WebSocket requires a dedicated token file; stdio requires owned pipes or sockets. Help and startup diagnostics use stderr; stdout belongs to the protocol."
    )]
    AppServer(crate::app_server::process::AppServerArgs),
}

#[derive(Debug, Subcommand)]
enum WorkflowCommand {
    Check(WorkflowArgs),
    Explain(WorkflowArgs),
}
#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Check(DiagnosticArgs),
    Show(ShowArgs),
}

/// Lexical selection fields shared by the static inspection commands.
#[derive(Debug, Default, Args)]
struct SelectionArgs {
    /// Select explicit model intent without editing authored defaults
    #[arg(long, value_parser = NonEmptyStringValueParser::new())]
    model: Option<String>,
    /// Replace the user rustx.toml source; native policy requires an absolute path
    #[arg(long)]
    config: Option<PathBuf>,
    /// Select the workspace whose rustx.toml is inspected
    #[arg(long)]
    workspace: Option<PathBuf>,
}
impl SelectionArgs {
    fn into_request(self) -> Result<LaunchRequest, ArgumentError> {
        Ok(LaunchRequest {
            model: self.model.as_deref().map(launch_text).transpose()?,
            config: self.config,
            workspace: self.workspace,
            ..Default::default()
        })
    }
}

#[derive(Debug, Args)]
struct DiagnosticArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    /// Bind runtime storage for this process; native policy requires an absolute path
    #[arg(long)]
    runtime_root: Option<PathBuf>,
    /// Render the native diagnostic report as JSON
    #[arg(long)]
    json: bool,
}
impl DiagnosticArgs {
    fn into_request(self) -> Result<LaunchRequest, ArgumentError> {
        Ok(LaunchRequest {
            runtime_root: self.runtime_root,
            ..self.selection.into_request()?
        })
    }
}
#[derive(Debug, Args)]
struct ShowArgs {
    #[command(flatten)]
    diagnostic: DiagnosticArgs,
    #[arg(long, required_unless_present = "agent", conflicts_with = "agent")]
    sources: bool,
    #[arg(long, value_parser = NonEmptyStringValueParser::new())]
    agent: Option<String>,
}
#[derive(Debug, Args)]
struct DoctorArgs {
    #[command(flatten)]
    diagnostic: DiagnosticArgs,
    /// Permit native probes after their effect plan has been printed
    #[arg(long, required = true)]
    probe: bool,
    /// Also permit managed Python preparation when the native probe plan requires it
    #[arg(long, requires = "probe")]
    prepare: bool,
}
#[derive(Debug, Args)]
struct WorkflowArgs {
    #[arg(value_parser = workflow_id)]
    id: crate::runtime::workflow::WorkflowId,
    #[command(flatten)]
    selection: SelectionArgs,
    /// Render the native diagnostic report as JSON
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TemplateArg {
    OpenaiChat,
    OpenaiResponses,
    Anthropic,
    Custom,
}
#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long, value_enum)]
    template: TemplateArg,
    #[arg(long, value_parser = NonEmptyStringValueParser::new())]
    provider: String,
    #[arg(long, value_parser = NonEmptyStringValueParser::new())]
    endpoint: String,
    #[arg(long, value_parser = NonEmptyStringValueParser::new())]
    credential_env: String,
    #[arg(long, value_parser = NonEmptyStringValueParser::new())]
    model_id: Option<String>,
    #[arg(long)]
    context_window: Option<u64>,
    #[arg(long)]
    max_output: Option<u32>,
    #[arg(long, action = clap::ArgAction::Set)]
    tool_calls: Option<bool>,
    #[arg(long, action = clap::ArgAction::Set)]
    reasoning: Option<bool>,
    /// Explicit TOML compatibility document; no compatibility is inferred
    #[arg(long)]
    compat: Option<String>,
    #[arg(long)]
    model_document: Option<PathBuf>,
    /// Render the native diagnostic report as JSON
    #[arg(long)]
    json: bool,
}
impl InitArgs {
    fn into_request(self) -> InitializationRequest {
        InitializationRequest {
            template: match self.template {
                TemplateArg::OpenaiChat => Template::OpenaiChat,
                TemplateArg::OpenaiResponses => Template::OpenaiResponses,
                TemplateArg::Anthropic => Template::Anthropic,
                TemplateArg::Custom => Template::Custom,
            },
            provider: self.provider,
            endpoint: self.endpoint,
            credential_env: self.credential_env,
            model_id: self.model_id,
            context_window: self.context_window,
            max_output: self.max_output,
            tool_calls: self.tool_calls,
            reasoning: self.reasoning,
            compat: self.compat,
            model_document: self.model_document,
        }
    }
}

/// A lexical or selection failure, with stream and exit policy left to rustX.
#[derive(Debug)]
pub struct ArgumentError(String);
impl std::fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ArgumentError {}

/// Parse public argv without printing, exiting, or capturing the host.
/// # Errors
/// Returns lexical or selection failures; accepted diagnostic intent owns JSON.
pub fn parse_command(
    arguments: impl IntoIterator<Item = impl Into<OsString> + Clone>,
) -> Result<Command, ArgumentError> {
    let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
    // A bare invocation is a help request, never an implicit service. A
    // subcommand missing its own subcommand stays a usage error.
    let bare = arguments.is_empty();
    let parsed =
        match Cli::try_parse_from(std::iter::once(OsString::from("rustx")).chain(arguments)) {
            Ok(parsed) => parsed,
            Err(error)
                if error.kind() == clap::error::ErrorKind::DisplayHelp
                    || (bare && error.kind()
                        == clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand) =>
            {
                return Ok(Command::Help(error.to_string()));
            }
            Err(error) => return Err(ArgumentError(error.to_string())),
        };
    Ok(match parsed.command {
        PublicCommand::AppServer(args) => Command::AppServer(args.into_request()),
        PublicCommand::Init(args) => Command::Init {
            json: args.json,
            request: args.into_request(),
        },
        PublicCommand::Config {
            command: ConfigCommand::Check(args),
        } => Command::Check {
            json: args.json,
            request: args.into_request()?,
        },
        PublicCommand::Config {
            command: ConfigCommand::Show(args),
        } => Command::Show {
            json: args.diagnostic.json,
            agent: args.agent,
            request: args.diagnostic.into_request()?,
        },
        PublicCommand::Doctor(args) => Command::Doctor {
            json: args.diagnostic.json,
            prepare: args.prepare,
            request: args.diagnostic.into_request()?,
        },
        PublicCommand::Workflow { command } => {
            let explain = matches!(command, WorkflowCommand::Explain(_));
            let (WorkflowCommand::Check(args) | WorkflowCommand::Explain(args)) = command;
            Command::Workflow {
                id: args.id,
                explain,
                json: args.json,
                request: args.selection.into_request()?,
            }
        }
    })
}

// Model selection text policy, applied only when converting exact CLI strings
// into LaunchRequest. Paths, Init and App Server never use it.
fn launch_text(value: &str) -> Result<String, ArgumentError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ArgumentError("model selection must not be blank".into()))
    } else {
        Ok(value.into())
    }
}
fn workflow_id(value: &str) -> Result<crate::runtime::workflow::WorkflowId, String> {
    crate::runtime::workflow::WorkflowId::parse(value)
        .map_err(|_| "invalid workflow identity".into())
}

#[cfg(test)]
mod tests;
