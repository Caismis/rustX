//! The bounded startup argument contract of the `rustx` binary.
//!
//! Parsing preserves user intent. The Rust launch resolver owns discovery,
//! field authority, trust, layering, defaults, paths and semantic validation.
//!
//! ```text
//! rustx [--models <path>] [--config <path>] [--workspace <dir>] [--runtime-root <dir>]
//!       [--model <provider/model>] [--trust grant|revoke]
//!       [--inspect-conversation <conversation-id>]
//!       [--continue | --session <session-id> [--node <node-id>]] [--name <text>]
//! ```
//!
//! Startup does not resume by itself: without a Session request the process
//! begins on an empty Session and the catalog's previous Sessions stay
//! reachable through `/resume`. There are exactly two ways to ask for a
//! persisted one, and both are explicit:
//!
//! - `--continue` binds the catalog's published active Session/node, which is
//!   also how a client completes a Session switch that required a process
//!   replacement;
//! - `--session <session-id>` (optionally `--node <node-id>`) names a
//!   persisted Session and makes that selection active — the same catalog
//!   transition `/resume` commits. The selection is planned before the
//!   runtime is composed and published together with it, in one catalog
//!   transaction, so a launch that cannot compose the Session it named
//!   leaves the active selection untouched.
//!
//! The two are mutually exclusive: a launch either continues whatever was
//! last active or names its destination, never both. Choosing a Session
//! interactively is a client concern — the picker lives in the terminal
//! client, which turns a choice into `--session`/`/resume` — so this process
//! has no `--resume` flag of its own.
//!
//! `--inspect-conversation` is a read-only generic conversation attachment.
//! It resolves the supplied identity to a running child's live Runtime Client
//! projection when available, otherwise to its durable authorities. It does
//! not compose a Session, model catalog, or execution runtime.
//!
//! `--name` is orthogonal to all of that: it names the Session the launch
//! bound, exactly as `/name` would once inside it. A name is display
//! metadata, never an identity, so no flag here ever resolves a Session by
//! one — `--session` takes the identity the catalog published, and nothing
//! else.

use std::path::PathBuf;

use super::composition::StartupSession;
use super::launch::{LaunchRequest, TrustAction};
use super::session::{SessionId, SessionNodeId};

const LAUNCH_VALUE_FLAGS: &[&str] = &[
    "--model",
    "--trust",
    "--models",
    "--config",
    "--workspace",
    "--runtime-root",
    "--skill",
    "--tools",
    "--exclude-tools",
    "--session",
    "--node",
    "--name",
    "--inspect-conversation",
];

pub const CONFIG_USAGE: &str = "Workflow authoring (offline, no execution or authority changes):\n\
  rustx workflow check <id> [--workspace <dir>] [--config <path>]\n\
    [--models <path>] [--model <provider/model>] [--json]\n\
  rustx workflow explain <id> [--workspace <dir>] [--config <path>]\n\
    [--models <path>] [--model <provider/model>] [--json]\n\
  Only configured Workflow ids are inspected. Static validity is not runtime readiness.\n\
  Exit 2: invalid; exit 3: static valid/incomplete, runtime readiness unresolved.\n\
Configuration commands:\n\
  rustx init --template openai-chat|openai-responses|anthropic|custom\n\
    --provider <id> --endpoint <url> --credential-env <NAME>\n\
    --model-id <id> --context-window <tokens> --max-output <tokens>\n\
    --tool-calls true|false --reasoning true|false [--compat <toml-document>] [--json]\n\
  custom replaces model flags with --model-document <path>.\n\
  OpenAI templates require explicit --compat; no compatibility is inferred.\n\
  rustx config check [launch selection/path flags] [--json]\n\
  rustx config show --sources [launch selection/path flags] [--json]\n\
  rustx doctor --probe [--prepare] [launch selection/path flags] [--json]\n\
Diagnostic commands reject Session and trust-change flags.\n\
Exit: 0 initialization/help complete; 1 probe or output failure; 2 invalid;\n\
3 incomplete or unresolved readiness. Check/show never resolve credentials,\n\
spawn, connect, prepare environments, or create Sessions/state.\n\
Show describes the prospective next launch. Doctor prints an effect plan before\n\
effects; --prepare explicitly permits managed Python preparation.\n\
Init creates user models.toml and settings.toml only, never overwrites files,\n\
and never grants project trust. Project rustx.toml remains optional.";

/// The finite Rust-owned command grammar. Runtime flags have one parser.
#[derive(Debug)]
pub enum Command {
    Workflow {
        id: crate::runtime::workflow::WorkflowId,
        explain: bool,
        request: LaunchRequest,
        json: bool,
    },
    Launch(LaunchRequest),
    Help,
    Check {
        request: LaunchRequest,
        json: bool,
    },
    Show {
        request: LaunchRequest,
        json: bool,
    },
    Doctor {
        request: LaunchRequest,
        json: bool,
        prepare: bool,
    },
    Init {
        arguments: Vec<String>,
        json: bool,
    },
}

impl Command {
    pub(super) const fn json_output(&self) -> bool {
        match self {
            Self::Workflow { json, .. }
            | Self::Check { json, .. }
            | Self::Show { json, .. }
            | Self::Doctor { json, .. }
            | Self::Init { json, .. } => *json,
            Self::Launch(_) | Self::Help => false,
        }
    }
}

pub(super) fn diagnostic_json_requested(arguments: &[String]) -> bool {
    matches!(
        arguments.first().map(String::as_str),
        Some("config" | "init" | "doctor" | "workflow")
    ) && remove_switch(&mut arguments.to_vec(), "--json").unwrap_or(true)
}

/// Route product commands before runtime admission.
///
/// # Errors
/// Rejects unknown commands, incompatible flags, and malformed launch intent.
pub fn parse_command(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Command, ArgumentError> {
    let mut arguments: Vec<_> = arguments.into_iter().collect();
    if arguments == ["--help"] {
        return Ok(Command::Help);
    }
    let Some(first) = arguments.first().cloned() else {
        return Ok(Command::Launch(LaunchRequest::default()));
    };
    if first == "workflow" {
        return parse_workflow(arguments);
    }
    if !matches!(first.as_str(), "init" | "config" | "doctor") {
        return parse_arguments(arguments).map(Command::Launch);
    }
    arguments.remove(0);
    let operation = if first == "config" {
        if arguments.is_empty() {
            return Err(ArgumentError::MissingValue {
                flag: "config".into(),
            });
        }
        let operation = arguments.remove(0);
        if !matches!(operation.as_str(), "check" | "show") {
            return Err(ArgumentError::UnknownFlag { flag: operation });
        }
        operation
    } else {
        first
    };
    let json = remove_switch(&mut arguments, "--json")?;
    if operation == "init" {
        return Ok(Command::Init { arguments, json });
    }
    let prepare = if operation == "doctor" {
        if !remove_switch(&mut arguments, "--probe")? {
            return Err(ArgumentError::MissingValue {
                flag: "doctor --probe".into(),
            });
        }
        remove_switch(&mut arguments, "--prepare")?
    } else {
        false
    };
    if operation == "show" && !remove_switch(&mut arguments, "--sources")? {
        return Err(ArgumentError::MissingValue {
            flag: "config show --sources".into(),
        });
    }
    let request = parse_arguments(arguments)?;
    if request.trust.is_some()
        || request.startup_session != StartupSession::Empty
        || request.session_name.is_some()
    {
        return Err(ArgumentError::InvalidValue {
            flag: "diagnostic commands do not accept trust or Session operations".into(),
        });
    }
    match operation.as_str() {
        "check" => Ok(Command::Check { request, json }),
        "show" => Ok(Command::Show { request, json }),
        "doctor" => Ok(Command::Doctor {
            request,
            json,
            prepare,
        }),
        _ => Err(ArgumentError::UnknownFlag { flag: operation }),
    }
}

fn parse_workflow(mut arguments: Vec<String>) -> Result<Command, ArgumentError> {
    if arguments.len() < 3 {
        return Err(ArgumentError::MissingValue {
            flag: "workflow check|explain <id>".into(),
        });
    }
    let explain = match arguments[1].as_str() {
        "check" => false,
        "explain" => true,
        _ => {
            return Err(ArgumentError::UnknownFlag {
                flag: arguments[1].clone(),
            });
        }
    };
    let id = crate::runtime::workflow::WorkflowId::parse(&arguments[2]).map_err(|_| {
        ArgumentError::InvalidValue {
            flag: "workflow id".into(),
        }
    })?;
    arguments.drain(..3);
    let json = remove_switch(&mut arguments, "--json")?;
    let mut index = 0;
    while index < arguments.len() {
        if !matches!(
            arguments[index].as_str(),
            "--workspace" | "--config" | "--models" | "--model"
        ) {
            return Err(ArgumentError::UnknownFlag {
                flag: arguments[index].clone(),
            });
        }
        index += 2;
    }
    let request = parse_arguments(arguments)?;
    Ok(Command::Workflow {
        id,
        explain,
        request,
        json,
    })
}

fn remove_switch(arguments: &mut Vec<String>, flag: &str) -> Result<bool, ArgumentError> {
    let mut count = 0;
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == flag {
            count += 1;
            arguments.remove(index);
        } else if LAUNCH_VALUE_FLAGS.contains(&arguments[index].as_str())
            || super::initialization::VALUE_FLAGS.contains(&arguments[index].as_str())
        {
            index += 2;
        } else {
            index += 1;
        }
    }
    if count > 1 {
        return Err(ArgumentError::Repeated { flag: flag.into() });
    }
    Ok(count == 1)
}

/// The usage text printed to **stderr** for an argument failure.
pub const USAGE: &str = "usage: rustx [--models <models.toml>] [--config <rustx.toml>] \
                         [--workspace <dir>] [--runtime-root <dir>] \
                         [--model <provider/model>] [--trust grant|revoke] \
                         [--inspect-conversation <conversation-id>] \
                         [--continue | --session <session-id> [--node <node-id>]] \
                         [--name <text>] [--skill <path>] [--no-skills] \
                         [--no-tools | --tools <a,b> | --no-builtin-tools] \
                         [--exclude-tools <a,b>]\n\
Tool selection: defaults include Read. --tools is exact; exclusions subtract last.\n\
--no-tools exposes zero main-model Tools and conflicts with --tools, --exclude-tools,\n\
and --no-builtin-tools. --tools conflicts with --no-builtin-tools.\n\
Explicit lists must be non-empty, unique, known and unambiguous.\n\
Tool exposure does not disable source preparation; use source enabled:false.\n\
Lazy Skills are advertised only when this domain admits native Read.";

/// Parses the bounded startup arguments.
///
/// # Errors
///
/// Returns a bounded diagnostic for an unknown flag, a missing value, a
/// invalid value, or a Session request that combines `--continue`
/// with `--session`.
#[allow(clippy::too_many_lines)] // one bounded flag parser, preserving explicit presence
pub fn parse_arguments(
    arguments: impl IntoIterator<Item = String>,
) -> Result<LaunchRequest, ArgumentError> {
    let mut model = None;
    let mut trust = None;
    let mut models: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut runtime_root: Option<PathBuf> = None;
    let mut skill_paths = Vec::new();
    let mut no_skills = false;
    let mut no_builtin_tools = false;
    let mut no_tools = false;
    let mut continue_active_session = false;
    let mut inspect_conversation: Option<String> = None;
    let mut session: Option<String> = None;
    let mut node: Option<String> = None;
    let mut session_name: Option<String> = None;
    let mut tools = None;
    let mut exclude_tools = None;

    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            value_flag if LAUNCH_VALUE_FLAGS.contains(&value_flag) => {
                let Some(value) = arguments.next() else {
                    return Err(ArgumentError::MissingValue { flag });
                };
                match flag.as_str() {
                    "--model" => set_text(&mut model, &value, &flag)?,
                    "--trust" => set_text(&mut trust, &value, &flag)?,
                    "--models" => set_path(&mut models, value.as_str(), flag.as_str())?,
                    "--config" => set_path(&mut config, value.as_str(), flag.as_str())?,
                    "--workspace" => set_path(&mut workspace, value.as_str(), flag.as_str())?,
                    "--runtime-root" => set_path(&mut runtime_root, value.as_str(), flag.as_str())?,
                    "--skill" => {
                        if value.is_empty() {
                            return Err(ArgumentError::InvalidValue { flag });
                        }
                        skill_paths.push(PathBuf::from(value));
                    }
                    "--session" => set_text(&mut session, value.as_str(), flag.as_str())?,
                    "--node" => set_text(&mut node, value.as_str(), flag.as_str())?,
                    "--name" => set_text(&mut session_name, value.as_str(), flag.as_str())?,
                    "--inspect-conversation" => {
                        set_text(&mut inspect_conversation, value.as_str(), flag.as_str())?;
                    }
                    "--tools" => set_names(&mut tools, value.as_str(), flag.as_str())?,
                    "--exclude-tools" => {
                        set_names(&mut exclude_tools, value.as_str(), flag.as_str())?;
                    }
                    _ => unreachable!(),
                }
            }
            "--no-skills" => set_bool(&mut no_skills, flag.as_str())?,
            "--no-builtin-tools" => set_bool(&mut no_builtin_tools, flag.as_str())?,
            "--no-tools" => set_bool(&mut no_tools, flag.as_str())?,
            "--continue" => set_bool(&mut continue_active_session, flag.as_str())?,
            other => {
                return Err(ArgumentError::UnknownFlag {
                    flag: other.to_owned(),
                });
            }
        }
    }

    let selection = crate::capabilities::AgentActivation {
        no_tools,
        no_builtin_tools,
        tools: tools.clone(),
        exclude_tools: exclude_tools.clone().unwrap_or_default(),
        ..Default::default()
    };
    if let Some((first, second)) = selection.conflict() {
        return Err(ArgumentError::Conflicting { first, second });
    }
    let startup_session = startup_request(
        inspect_conversation,
        continue_active_session,
        session,
        node,
        session_name.is_some(),
    )?;
    let trust = match trust.as_deref() {
        None => None,
        Some("grant") => Some(TrustAction::Grant),
        Some("revoke") => Some(TrustAction::Revoke),
        Some(_) => {
            return Err(ArgumentError::InvalidValue {
                flag: "--trust".into(),
            });
        }
    };
    Ok(LaunchRequest {
        models,
        config,
        workspace,
        runtime_root,
        model,
        trust,
        skill_paths,
        no_skills,
        no_builtin_tools,
        no_tools,
        startup_session,
        session_name,
        tools,
        exclude_tools,
    })
}

fn startup_request(
    inspect_conversation: Option<String>,
    continue_active_session: bool,
    session: Option<String>,
    node: Option<String>,
    has_session_name: bool,
) -> Result<StartupSession, ArgumentError> {
    if let Some(conversation_id) = inspect_conversation {
        if continue_active_session {
            return Err(ArgumentError::Conflicting {
                first: "--inspect-conversation",
                second: "--continue",
            });
        }
        if session.is_some() {
            return Err(ArgumentError::Conflicting {
                first: "--inspect-conversation",
                second: "--session",
            });
        }
        if has_session_name {
            return Err(ArgumentError::Conflicting {
                first: "--inspect-conversation",
                second: "--name",
            });
        }
        if node.is_some() {
            return Err(ArgumentError::Conflicting {
                first: "--inspect-conversation",
                second: "--node",
            });
        }
        Ok(StartupSession::InspectConversation {
            conversation_id: crate::runtime::identity::ConversationId::new(conversation_id),
        })
    } else {
        startup_session(continue_active_session, session, node)
    }
}

/// Resolves the one Session this launch asks for.
///
/// The requests are mutually exclusive by construction: naming a destination
/// and continuing whatever was last active are different intentions, and
/// silently preferring one would make the other look honoured.
fn startup_session(
    continue_active_session: bool,
    session: Option<String>,
    node: Option<String>,
) -> Result<StartupSession, ArgumentError> {
    if let Some(session) = session {
        if continue_active_session {
            return Err(ArgumentError::Conflicting {
                first: "--continue",
                second: "--session",
            });
        }
        return Ok(StartupSession::Select {
            session: SessionId::new(session),
            node: node.map(SessionNodeId::new),
        });
    }
    if node.is_some() {
        return Err(ArgumentError::Dependent {
            flag: "--node",
            requires: "--session",
        });
    }
    if continue_active_session {
        Ok(StartupSession::ContinueActive)
    } else {
        Ok(StartupSession::Empty)
    }
}

/// Accepts one non-empty trimmed text value: an identity to resolve, or the
/// display name to give the Session this launch binds.
fn set_text(slot: &mut Option<String>, value: &str, flag: &str) -> Result<(), ArgumentError> {
    if slot.is_some() {
        return Err(ArgumentError::Repeated {
            flag: flag.to_owned(),
        });
    }
    let value = value.trim();
    if value.is_empty() {
        return Err(ArgumentError::InvalidValue {
            flag: flag.to_owned(),
        });
    }
    *slot = Some(value.to_owned());
    Ok(())
}

fn set_path(slot: &mut Option<PathBuf>, value: &str, flag: &str) -> Result<(), ArgumentError> {
    if slot.is_some() {
        return Err(ArgumentError::Repeated {
            flag: flag.to_owned(),
        });
    }
    if value.is_empty() {
        return Err(ArgumentError::InvalidValue { flag: flag.into() });
    }
    *slot = Some(PathBuf::from(value));
    Ok(())
}

fn set_names(slot: &mut Option<Vec<String>>, value: &str, flag: &str) -> Result<(), ArgumentError> {
    if slot.is_some() {
        return Err(ArgumentError::Repeated {
            flag: flag.to_owned(),
        });
    }
    let names = value
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    crate::capabilities::validate_tool_names(&names, flag)
        .map_err(|_| ArgumentError::InvalidValue { flag: flag.into() })?;
    *slot = Some(names);
    Ok(())
}

fn set_bool(slot: &mut bool, flag: &str) -> Result<(), ArgumentError> {
    if *slot {
        return Err(ArgumentError::Repeated {
            flag: flag.to_owned(),
        });
    }
    *slot = true;
    Ok(())
}

/// A bounded startup argument failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentError {
    /// An unrecognized flag was supplied.
    UnknownFlag {
        /// The offending flag.
        flag: String,
    },
    /// A flag was supplied without its value.
    MissingValue {
        /// The flag whose value is missing.
        flag: String,
    },
    /// A flag was supplied more than once.
    Repeated {
        /// The repeated flag.
        flag: String,
    },
    /// A value was syntactically present but empty or otherwise unusable.
    InvalidValue {
        /// The flag whose value was invalid.
        flag: String,
    },
    /// Two flags that express different intentions were combined.
    Conflicting {
        /// The first of the combined flags.
        first: &'static str,
        /// The second of the combined flags.
        second: &'static str,
    },
    /// A flag was supplied without the flag it qualifies.
    Dependent {
        /// The supplied flag.
        flag: &'static str,
        /// The flag it requires.
        requires: &'static str,
    },
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFlag { flag } => write!(f, "unknown argument {flag:?}"),
            Self::MissingValue { flag } => write!(f, "argument {flag} requires a value"),
            Self::Repeated { flag } => write!(f, "argument {flag} was supplied more than once"),
            Self::InvalidValue { flag } => write!(f, "argument {flag} requires a non-empty value"),
            Self::Conflicting { first, second } => {
                write!(f, "arguments {first} and {second} cannot be combined")
            }
            Self::Dependent { flag, requires } => {
                write!(f, "argument {flag} requires {requires}")
            }
        }
    }
}

impl std::error::Error for ArgumentError {}

#[cfg(test)]
mod tests {
    #[test]
    fn cfg237_workflow_grammar_is_finite_and_rejects_execution_controls() {
        use super::*;
        let parse = |args: &[&str]| parse_command(args.iter().map(ToString::to_string));
        for operation in ["check", "explain"] {
            assert!(matches!(
                parse(&[
                    "workflow",
                    operation,
                    "typed_agent",
                    "--json",
                    "--workspace",
                    "project"
                ])
                .unwrap(),
                Command::Workflow { json: true, .. }
            ));
            for flag in [
                "--trust",
                "--session",
                "--continue",
                "--name",
                "--runtime-root",
                "--prepare",
                "--probe",
                "--tools",
                "--no-tools",
                "--skill",
            ] {
                assert!(
                    parse(&["workflow", operation, "typed_agent", flag, "x"]).is_err(),
                    "{flag}"
                );
            }
            assert!(parse(&["workflow", operation, "typed_agent", "--json", "--json"]).is_err());
            assert!(parse(&["workflow", operation, "../arbitrary.yaml"]).is_err());
        }
        assert!(parse(&["workflow", "run", "typed_agent"]).is_err());
        assert!(parse(&["workflow", "check"]).is_err());
    }
    #[test]
    fn cfg235_finite_command_grammar_and_switch_values() {
        use super::{Command, parse_command};
        let parse = |flags: &[&str]| parse_command(flags.iter().map(ToString::to_string));
        assert!(matches!(parse(&["--help"]).unwrap(), Command::Help));
        assert!(matches!(
            parse(&["config", "check", "--json"]).unwrap(),
            Command::Check { json: true, .. }
        ));
        assert!(matches!(
            parse(&["config", "show", "--sources"]).unwrap(),
            Command::Show { json: false, .. }
        ));
        assert!(matches!(
            parse(&["doctor", "--probe", "--prepare", "--json"]).unwrap(),
            Command::Doctor {
                prepare: true,
                json: true,
                ..
            }
        ));
        for flags in [
            vec!["config"],
            vec!["config", "init"],
            vec!["config", "show"],
            vec!["doctor"],
            vec!["config", "check", "--prepare"],
            vec!["config", "check", "--trust", "grant"],
            vec!["config", "check", "--continue"],
            vec!["config", "check", "--name", "x"],
            vec!["config", "check", "--json", "--json"],
            vec!["doctor", "--probe", "--probe"],
        ] {
            assert!(parse(&flags).is_err(), "{flags:?}");
        }
        let Command::Check { request, json } =
            parse(&["config", "check", "--workspace", "--json"]).unwrap()
        else {
            panic!("check")
        };
        assert!(!json);
        assert_eq!(request.workspace, Some("--json".into()));
        for phrase in [
            "0 initialization",
            "1 probe",
            "2 invalid",
            "3 incomplete",
            "prospective next launch",
            "never overwrites",
        ] {
            assert!(super::CONFIG_USAGE.contains(phrase));
        }
    }

    use super::{ArgumentError, SessionId, SessionNodeId, StartupSession, parse_arguments};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// The complete argument set parses into explicit paths.
    #[test]
    fn complete_arguments_parse() {
        let paths = parse_arguments(args(&[
            "--models",
            "/m.json",
            "--config",
            "/r.json",
            "--workspace",
            "/ws",
            "--runtime-root",
            "/private",
        ]))
        .expect("valid");
        assert_eq!(
            paths.models.as_deref().and_then(std::path::Path::to_str),
            Some("/m.json")
        );
        assert_eq!(
            paths.config.as_deref().and_then(std::path::Path::to_str),
            Some("/r.json")
        );
    }

    /// Unknown, repeated, valueless, and missing arguments all fail
    /// explicitly.
    #[test]
    fn malformed_arguments_fail() {
        assert!(matches!(
            parse_arguments(args(&["--future"])).expect_err("unknown"),
            ArgumentError::UnknownFlag { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--models"])).expect_err("no value"),
            ArgumentError::MissingValue { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--models", "a", "--models", "b"])).expect_err("repeated"),
            ArgumentError::Repeated { .. }
        ));
        assert!(
            parse_arguments(args(&[]))
                .expect("minimal intent")
                .models
                .is_none()
        );
        // Choosing a Session interactively belongs to the client that can
        // draw a picker; this process only ever receives the choice.
        assert!(matches!(
            parse_arguments(args(&["--resume"])).expect_err("client concern"),
            ArgumentError::UnknownFlag { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--session", "  "])).expect_err("empty identity"),
            ArgumentError::InvalidValue { .. }
        ));
    }

    /// A launch does not resume by itself. The flag that asks for the
    /// catalog's published active Session is explicit, off by default, and
    /// rejected when it is repeated.
    #[test]
    fn continuing_the_active_session_is_an_explicit_startup_request() {
        let default = parse_arguments(args(&[
            "--models",
            "m",
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
        ]))
        .expect("defaults");
        assert_eq!(default.startup_session, StartupSession::Empty);

        let continued = parse_arguments(args(&[
            "--models",
            "m",
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
            "--continue",
        ]))
        .expect("continue");
        assert_eq!(continued.startup_session, StartupSession::ContinueActive);

        assert!(matches!(
            parse_arguments(args(&["--continue", "--continue"])).expect_err("repeated"),
            ArgumentError::Repeated { .. }
        ));
    }

    /// A launch can also name where it starts. The named Session — and,
    /// when given, the named lineage node — is carried through as an
    /// explicit selection request, and it cannot be combined with the
    /// request to continue whatever was last active.
    #[test]
    fn naming_a_startup_session_is_exclusive_and_carries_its_optional_node() {
        let base = args(&[
            "--models",
            "m",
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
        ]);
        let with = |extra: &[&str]| {
            let mut values = base.clone();
            values.extend(args(extra));
            values
        };

        let selected = parse_arguments(with(&["--session", "session-3"])).expect("session");
        assert_eq!(
            selected.startup_session,
            StartupSession::Select {
                session: SessionId::new("session-3"),
                node: None,
            }
        );

        let node = parse_arguments(with(&["--session", "session-3", "--node", "node-7"]))
            .expect("session and node");
        assert_eq!(
            node.startup_session,
            StartupSession::Select {
                session: SessionId::new("session-3"),
                node: Some(SessionNodeId::new("node-7")),
            }
        );

        assert!(matches!(
            parse_arguments(with(&["--session", "session-3", "--continue"])).expect_err("both"),
            ArgumentError::Conflicting {
                first: "--continue",
                second: "--session"
            }
        ));
        assert!(matches!(
            parse_arguments(with(&["--node", "node-7"])).expect_err("unqualified node"),
            ArgumentError::Dependent {
                flag: "--node",
                requires: "--session"
            }
        ));
        assert!(matches!(
            parse_arguments(with(&["--session", "a", "--session", "b"])).expect_err("repeated"),
            ArgumentError::Repeated { .. }
        ));
    }

    /// `--name` is display metadata, so it qualifies whichever Session the
    /// launch bound rather than choosing one: it combines with every startup
    /// Session request, including none at all, and it is never a way to say
    /// *which* Session to open.
    #[test]
    fn naming_the_bound_session_combines_with_every_startup_session_request() {
        let base = args(&[
            "--models",
            "m",
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
        ]);
        let with = |extra: &[&str]| {
            let mut values = base.clone();
            values.extend(args(extra));
            values
        };

        let empty = parse_arguments(with(&["--name", "  auth refactor  "])).expect("name");
        assert_eq!(empty.session_name.as_deref(), Some("auth refactor"));
        assert_eq!(empty.startup_session, StartupSession::Empty);

        let continued =
            parse_arguments(with(&["--continue", "--name", "auth refactor"])).expect("name");
        assert_eq!(continued.session_name.as_deref(), Some("auth refactor"));
        assert_eq!(continued.startup_session, StartupSession::ContinueActive);

        let selected =
            parse_arguments(with(&["--session", "session-3", "--name", "auth refactor"]))
                .expect("name");
        assert_eq!(selected.session_name.as_deref(), Some("auth refactor"));
        assert_eq!(
            selected.startup_session,
            StartupSession::Select {
                session: SessionId::new("session-3"),
                node: None,
            }
        );

        assert_eq!(
            parse_arguments(base.clone()).expect("no name").session_name,
            None
        );
        assert!(matches!(
            parse_arguments(with(&["--name", "   "])).expect_err("empty name"),
            ArgumentError::InvalidValue { .. }
        ));
        assert!(matches!(
            parse_arguments(with(&["--name", "a", "--name", "b"])).expect_err("repeated"),
            ArgumentError::Repeated { .. }
        ));
    }

    /// A known conversation identity selects the generic read-only startup
    /// path exactly, and never falls through to Session composition.
    #[test]
    fn inspecting_a_conversation_is_an_exclusive_startup_request() {
        let base = args(&[
            "--models",
            "m",
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
        ]);
        let with = |extra: &[&str]| {
            let mut values = base.clone();
            values.extend(args(extra));
            values
        };

        let inspected =
            parse_arguments(with(&["--inspect-conversation", "conv-parent-subagent-1"]))
                .expect("inspection");
        assert_eq!(
            inspected.startup_session,
            StartupSession::InspectConversation {
                conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv-parent-subagent-1",
                ),
            }
        );
        assert!(matches!(
            parse_arguments(with(&["--inspect-conversation", "child", "--continue"]))
                .expect_err("inspection and continue"),
            ArgumentError::Conflicting {
                first: "--inspect-conversation",
                second: "--continue"
            }
        ));
        assert!(matches!(
            parse_arguments(with(&["--inspect-conversation", "child", "--session", "s"]))
                .expect_err("inspection and session"),
            ArgumentError::Conflicting {
                first: "--inspect-conversation",
                second: "--session"
            }
        ));
        assert!(matches!(
            parse_arguments(with(&[
                "--inspect-conversation",
                "child",
                "--name",
                "label"
            ]))
            .expect_err("inspection and name"),
            ArgumentError::Conflicting {
                first: "--inspect-conversation",
                second: "--name"
            }
        ));
    }

    #[test]
    fn tool_and_skill_startup_options_are_typed_and_repeatable_only_where_allowed() {
        let paths = parse_arguments(args(&[
            "--models",
            "m",
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
            "--no-skills",
            "--skill",
            "one",
            "--skill",
            "two",
            "--tools",
            "read, search",
            "--exclude-tools",
            "bash,grep",
        ]))
        .expect("options");
        assert!(paths.no_skills);
        assert_eq!(
            paths.skill_paths,
            vec![
                std::path::PathBuf::from("one"),
                std::path::PathBuf::from("two")
            ]
        );
        assert_eq!(
            paths.tools,
            Some(vec!["read".to_owned(), "search".to_owned()])
        );
        assert_eq!(
            paths.exclude_tools,
            Some(vec!["bash".to_owned(), "grep".to_owned()])
        );
        assert!(!paths.no_builtin_tools);
    }

    #[test]
    fn exact_tool_flag_conflicts_and_explicit_lists_fail_closed() {
        for flags in [
            vec!["--no-tools", "--tools", "read"],
            vec!["--no-tools", "--exclude-tools", "read"],
            vec!["--no-tools", "--no-builtin-tools"],
            vec!["--tools", "read", "--no-builtin-tools"],
        ] {
            assert!(matches!(
                parse_arguments(args(&flags)),
                Err(ArgumentError::Conflicting { .. })
            ));
        }
        for flag in ["--tools", "--exclude-tools"] {
            for value in [
                "",
                " ",
                ",",
                "read,",
                ",read",
                "read,,grep",
                "read,read",
                "read, read",
            ] {
                assert!(
                    matches!(
                        parse_arguments(args(&[flag, value])),
                        Err(ArgumentError::InvalidValue { .. })
                    ),
                    "{flag} {value:?}"
                );
            }
        }
        for flags in [
            vec!["--no-tools"],
            vec!["--tools", "read,grep", "--exclude-tools", "read"],
            vec!["--no-builtin-tools", "--exclude-tools", "external"],
        ] {
            assert!(parse_arguments(args(&flags)).is_ok());
        }
    }
}
