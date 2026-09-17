//! The bounded startup argument contract of the `rustx` binary.
//!
//! Parsing preserves user intent. The Rust launch resolver owns discovery,
//! field authority, semantic overlay, defaults, paths and semantic validation.
//!
//! ```text
//! rustx [--config <path>] [--workspace <dir>] [--runtime-root <dir>]
//!       [--model <model-name>]
//!       [--inspect-conversation <conversation-id>]
//!       [--session <session-id> [--node <node-id>]] [--name <text>]
//! ```
//!
//! Startup does not resume by itself: without a Session request the process
//! begins on an empty Session and the catalog's previous Sessions stay
//! reachable through `/resume`. `--session <session-id>` with optional
//! `--node <node-id>` names the persisted attachment explicitly. There is no
//! catalog-global focus and `--continue` is rejected. Choosing an identity
//! interactively belongs to the terminal client, so the native binary has no
//! `--resume` flag of its own.
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
use super::launch::LaunchRequest;
use super::session::{SessionId, SessionNodeId};

const LAUNCH_VALUE_FLAGS: &[&str] = &[
    "--model",
    "--config",
    "--workspace",
    "--runtime-root",
    "--session",
    "--node",
    "--name",
    "--inspect-conversation",
];

pub const CONFIG_USAGE: &str = "Workflow authoring (offline, no execution or authority changes):\n\
  rustx workflow check <id> [--workspace <dir>] [--config <path>]\n\
    [--model <model-name>] [--json]\n\
  rustx workflow explain <id> [--workspace <dir>] [--config <path>]\n\
    [--model <model-name>] [--json]\n\
  Only discovered Workflow ids are inspected. Static validity is not runtime readiness.\n\
  Exit 2: invalid; exit 3: static valid/incomplete, runtime readiness unresolved.\n\
Configuration commands:\n\
  rustx init --template openai-chat|openai-responses|anthropic|custom\n\
    --provider <id> --endpoint <url> --credential-env <NAME>\n\
    --model-id <id> --context-window <tokens> --max-output <tokens>\n\
    --tool-calls true|false --reasoning true|false [--compat <toml-document>] [--json]\n\
  custom replaces model flags with --model-document <path>.\n\
  OpenAI templates require explicit --compat; no compatibility is inferred.\n\
  rustx config check [launch selection/path flags] [--json]\n\
  rustx config show (--sources | --agent <main|name>) [launch selection/path flags] [--json]\n\
  rustx doctor --probe [--prepare] [launch selection/path flags] [--json]\n\
Diagnostic commands reject Session mutation flags.\n\
Exit: 0 initialization/help complete; 1 probe or output failure; 2 invalid;\n\
3 incomplete or unresolved readiness. Check/show never resolve credentials,\n\
spawn, connect, prepare environments, or create Sessions/state.\n\
Show describes the prospective next launch. Doctor prints an effect plan before\n\
effects; --prepare explicitly permits managed Python preparation.\n\
Init creates ~/rustx/rustx.toml and ~/rustx/.agents, never overwrites files,\n\
Workspace rustx.toml remains optional.";

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
    let mut agent = None;
    if operation == "show" {
        let sources = remove_switch(&mut arguments, "--sources")?;
        if let Some(index) = arguments.iter().position(|value| value == "--agent") {
            arguments.remove(index);
            if index == arguments.len() || arguments[index].starts_with("--") {
                return Err(ArgumentError::MissingValue {
                    flag: "--agent".into(),
                });
            }
            agent = Some(arguments.remove(index));
        }
        if sources && agent.is_some() {
            return Err(ArgumentError::Conflicting {
                first: "--sources",
                second: "--agent",
            });
        }
        if !sources && agent.is_none() {
            return Err(ArgumentError::MissingValue {
                flag: "config show --sources or --agent".into(),
            });
        }
    }
    let request = parse_arguments(arguments)?;
    if request.startup_session != StartupSession::Empty || request.session_name.is_some() {
        return Err(ArgumentError::InvalidValue {
            flag: "diagnostic commands do not accept Session operations".into(),
        });
    }
    match operation.as_str() {
        "check" => Ok(Command::Check { request, json }),
        "show" => Ok(Command::Show {
            request,
            json,
            agent,
        }),
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
            "--workspace" | "--config" | "--model"
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
pub const USAGE: &str = r"usage: rustx [--config <absolute-rustx.toml>] [--workspace <dir>]
             [--runtime-root <absolute-dir>] [--model <model-name>]
             [--inspect-conversation <conv_uuid-v7>]
             [--session <ses_uuid-v7> [--node <node_uuid-v7>]] [--name <text>]

User configuration defaults to ~/rustx/rustx.toml. --config replaces only that
User source binding; it never relocates ~/rustx/.agents or ~/rustx/runtime.
Workspace configuration is <workspace>/rustx.toml, with no ancestor accumulation.
--runtime-root is a process binding; reload cannot change it.
--model selects deliberate Session intent and never edits an authored default.

rustx.toml owns Root capability selection and global Tool invocation policy.
Root Native Tools are an explicit whitelist. Skills, MCP and Managed Python
use all/exact/empty selections. Plugins default off. Named Agents own complete
independent profiles; Root authorizes delegation through agent.agents.
Resource definitions in the two .agents roots grant no Root capabilities.
Unused MCP and Python definitions do not connect or prepare environments.

Skills are prompt visibility, never filesystem access control. Prompts provide
the User and Workspace Skill roots for progressive disclosure. Same-name
Workspace resources shadow User resources completely, including invalid ones.
Save commits source bytes only. /reload publishes one coherent configuration
generation; already-admitted work retains its frozen generation.
";

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
    let mut config: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut runtime_root: Option<PathBuf> = None;
    let mut continue_active_session = false;
    let mut inspect_conversation: Option<String> = None;
    let mut session: Option<String> = None;
    let mut node: Option<String> = None;
    let mut session_name: Option<String> = None;

    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            value_flag if LAUNCH_VALUE_FLAGS.contains(&value_flag) => {
                let Some(value) = arguments.next() else {
                    return Err(ArgumentError::MissingValue { flag });
                };
                match flag.as_str() {
                    "--model" => set_text(&mut model, &value, &flag)?,
                    "--config" => set_path(&mut config, value.as_str(), flag.as_str())?,
                    "--workspace" => set_path(&mut workspace, value.as_str(), flag.as_str())?,
                    "--runtime-root" => set_path(&mut runtime_root, value.as_str(), flag.as_str())?,
                    "--session" => set_text(&mut session, value.as_str(), flag.as_str())?,
                    "--node" => set_text(&mut node, value.as_str(), flag.as_str())?,
                    "--name" => set_text(&mut session_name, value.as_str(), flag.as_str())?,
                    "--inspect-conversation" => {
                        set_text(&mut inspect_conversation, value.as_str(), flag.as_str())?;
                    }
                    _ => unreachable!(),
                }
            }
            "--continue" => set_bool(&mut continue_active_session, flag.as_str())?,
            other => {
                return Err(ArgumentError::UnknownFlag {
                    flag: other.to_owned(),
                });
            }
        }
    }

    let startup_session = startup_request(
        inspect_conversation,
        continue_active_session,
        session,
        node,
        session_name.is_some(),
    )?;
    Ok(LaunchRequest {
        config,
        workspace,
        runtime_root,
        model,
        startup_session,
        session_name,
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
            conversation_id: crate::runtime::identity::ConversationId::parse(conversation_id)
                .map_err(|_| ArgumentError::InvalidValue {
                    flag: "--inspect-conversation".into(),
                })?,
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
            session: SessionId::parse(session).map_err(|_| ArgumentError::InvalidValue {
                flag: "--session".into(),
            })?,
            node: node.map(SessionNodeId::parse).transpose().map_err(|_| {
                ArgumentError::InvalidValue {
                    flag: "--node".into(),
                }
            })?,
        });
    }
    if node.is_some() {
        return Err(ArgumentError::Dependent {
            flag: "--node",
            requires: "--session",
        });
    }
    if continue_active_session {
        Err(ArgumentError::Dependent {
            flag: "--continue",
            requires: "--session",
        })
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
                "--session",
                "--continue",
                "--name",
                "--runtime-root",
                "--prepare",
                "--probe",
                "--no-direct-tools",
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
    fn cfg275_show_requires_exactly_one_target() {
        use super::{Command, parse_command};
        let parse = |args: &[&str]| parse_command(args.iter().map(ToString::to_string));
        for args in [
            vec!["--sources"],
            vec!["--agent", "main"],
            vec!["--agent", "reviewer"],
        ] {
            assert!(matches!(
                parse(&[vec!["config", "show"], args].concat()),
                Ok(Command::Show { .. })
            ));
        }
        for args in [
            vec!["--sources", "--agent", "main"],
            vec!["--agent", "main", "--sources"],
        ] {
            assert!(matches!(
                parse(&[vec!["config", "show"], args].concat()),
                Err(ArgumentError::Conflicting {
                    first: "--sources",
                    second: "--agent"
                })
            ));
        }
        for args in [vec![], vec!["--agent"], vec!["--agent", "--json"]] {
            assert!(matches!(
                parse(&[vec!["config", "show"], args].concat()),
                Err(ArgumentError::MissingValue { .. })
            ));
        }
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
            "--config",
            "/r.json",
            "--workspace",
            "/ws",
            "--runtime-root",
            "/private",
        ]))
        .expect("valid");
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
            parse_arguments(args(&["--future"]))
                .expect_err("ses_b23a6a84-39c0-7de5-8158-93e7c90c1e32"),
            ArgumentError::UnknownFlag { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--config"])).expect_err("no value"),
            ArgumentError::MissingValue { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--config", "a", "--config", "b"])).expect_err("repeated"),
            ArgumentError::Repeated { .. }
        ));
        assert!(
            parse_arguments(args(&[]))
                .expect("minimal intent")
                .config
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

    /// A launch creates a Session by default; obsolete implicit resume is rejected.
    #[test]
    fn implicit_resume_is_rejected_without_a_session_identity() {
        let default = parse_arguments(args(&[
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
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
            "--continue",
        ]))
        .expect_err("implicit global resume is removed");
        assert!(matches!(continued, ArgumentError::Dependent { .. }));

        assert!(matches!(
            parse_arguments(args(&["--continue", "--continue"])).expect_err("repeated"),
            ArgumentError::Repeated { .. }
        ));
    }

    /// A launch can also name where it starts. The named Session — and,
    /// when given, the named lineage node — is carried through as an
    /// explicit attachment request. The obsolete implicit-resume flag is rejected.
    #[test]
    fn naming_a_startup_session_is_exclusive_and_carries_its_optional_node() {
        let base = args(&["--config", "r", "--workspace", "w", "--runtime-root", "p"]);
        let with = |extra: &[&str]| {
            let mut values = base.clone();
            values.extend(args(extra));
            values
        };

        let selected = parse_arguments(with(&[
            "--session",
            "ses_eb278475-f606-7143-87df-8cb657e1c7ee",
        ]))
        .expect("session");
        assert_eq!(
            selected.startup_session,
            StartupSession::Select {
                session: SessionId::new("ses_eb278475-f606-7143-87df-8cb657e1c7ee"),
                node: None,
            }
        );

        let node = parse_arguments(with(&[
            "--session",
            "ses_eb278475-f606-7143-87df-8cb657e1c7ee",
            "--node",
            "node_c346d387-9a21-70f0-8e5c-7422521183b3",
        ]))
        .expect("session and node");
        assert_eq!(
            node.startup_session,
            StartupSession::Select {
                session: SessionId::new("ses_eb278475-f606-7143-87df-8cb657e1c7ee"),
                node: Some(SessionNodeId::new(
                    "node_c346d387-9a21-70f0-8e5c-7422521183b3"
                )),
            }
        );

        assert!(matches!(
            parse_arguments(with(&[
                "--session",
                "ses_eb278475-f606-7143-87df-8cb657e1c7ee",
                "--continue"
            ]))
            .expect_err("both"),
            ArgumentError::Conflicting {
                first: "--continue",
                second: "--session"
            }
        ));
        assert!(matches!(
            parse_arguments(with(&[
                "--node",
                "node_c346d387-9a21-70f0-8e5c-7422521183b3"
            ]))
            .expect_err("unqualified node"),
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
        let base = args(&["--config", "r", "--workspace", "w", "--runtime-root", "p"]);
        let with = |extra: &[&str]| {
            let mut values = base.clone();
            values.extend(args(extra));
            values
        };

        let empty = parse_arguments(with(&["--name", "  auth refactor  "])).expect("name");
        assert_eq!(empty.session_name.as_deref(), Some("auth refactor"));
        assert_eq!(empty.startup_session, StartupSession::Empty);

        let continued = parse_arguments(with(&["--continue", "--name", "auth refactor"]))
            .expect_err("explicit identity required");
        assert!(matches!(continued, ArgumentError::Dependent { .. }));

        let selected = parse_arguments(with(&[
            "--session",
            "ses_eb278475-f606-7143-87df-8cb657e1c7ee",
            "--name",
            "auth refactor",
        ]))
        .expect("name");
        assert_eq!(selected.session_name.as_deref(), Some("auth refactor"));
        assert_eq!(
            selected.startup_session,
            StartupSession::Select {
                session: SessionId::new("ses_eb278475-f606-7143-87df-8cb657e1c7ee"),
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
        let base = args(&["--config", "r", "--workspace", "w", "--runtime-root", "p"]);
        let with = |extra: &[&str]| {
            let mut values = base.clone();
            values.extend(args(extra));
            values
        };

        let inspected = parse_arguments(with(&[
            "--inspect-conversation",
            "conv_c89c766b-0004-76cb-8baf-0d5b1a677c1e",
        ]))
        .expect("inspection");
        assert_eq!(
            inspected.startup_session,
            StartupSession::InspectConversation {
                conversation_id: crate::runtime::identity::ConversationId::new(
                    "conv_c89c766b-0004-76cb-8baf-0d5b1a677c1e"
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
    fn cfg332_removed_configuration_and_session_narrowing_flags_are_rejected() {
        for flag in [
            "--models",
            "--user-settings",
            "--no-automatic-skills",
            "--skill",
            "--skills",
            "--no-builtin-tools",
            "--no-direct-tools",
            "--tools",
            "--exclude-tools",
            "--trust",
        ] {
            assert!(
                matches!(
                    parse_arguments(args(&[flag])),
                    Err(ArgumentError::UnknownFlag { .. })
                ),
                "{flag}"
            );
        }
    }
}
