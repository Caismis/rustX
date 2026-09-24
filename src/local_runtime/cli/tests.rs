use super::*;
fn parse(args: &[&str]) -> Result<Command, ArgumentError> {
    parse_command(args.iter().map(ToString::to_string))
}

#[test]
fn cli01_grammar_is_consistent_and_every_command_converts() {
    use clap::CommandFactory;
    Cli::command().debug_assert();
    for args in [
        vec![],
        vec!["config", "check"],
        vec!["config", "show", "--sources"],
        vec!["config", "show", "--agent", "main"],
        vec!["doctor", "--probe"],
        vec!["workflow", "check", "review"],
        vec!["workflow", "explain", "review"],
        vec!["app-server", "--listen", "stdio"],
        vec![
            "init",
            "--template",
            "custom",
            "--provider",
            "local",
            "--endpoint",
            "http://localhost",
            "--credential-env",
            "KEY",
        ],
    ] {
        assert!(parse(&args).is_ok(), "{args:?}");
    }
}
#[test]
fn cli02_lexical_failures_and_cli03_finite_permissions() {
    for args in [
        vec!["--config"],
        vec!["--config="],
        vec!["--name", "  "],
        vec!["--config", "a", "--config", "b"],
        vec!["--future"],
        vec!["--continue"],
        vec!["--session", "bad"],
        vec!["--node", "node_c346d387-9a21-70f0-8e5c-7422521183b3"],
        vec!["config"],
        vec!["config", "show"],
        vec!["config", "show", "--sources", "--agent", "main"],
        vec!["config", "check", "--json", "--json"],
        vec!["config", "check", "--name", "x"],
        vec!["config", "check", "--inspect-conversation", "x"],
        vec!["doctor"],
        vec!["doctor", "--prepare"],
        vec!["doctor", "--probe", "--probe"],
        vec!["workflow", "run", "x"],
        vec!["workflow", "check"],
        vec!["workflow", "check", "../x"],
        vec!["app-server"],
        vec!["app-server", "--listen=stdio", "--model=x"],
        vec!["app-server", "--listen="],
        vec!["app-server", "--listen=stdio", "--listen=stdio"],
        vec!["--model=x", "config", "check"],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    for operation in ["check", "explain"] {
        for flag in [
            "--session",
            "--name",
            "--runtime-root",
            "--probe",
            "--prepare",
        ] {
            assert!(parse(&["workflow", operation, "review", flag, "x"]).is_err());
        }
    }
    assert!(matches!(
        parse(&["doctor", "--probe", "--prepare"]).unwrap(),
        Command::Doctor { prepare: true, .. }
    ));
}
#[test]
fn cli04_omission_and_identity_conversion() {
    let Command::Launch(request) = parse(&[]).unwrap() else {
        panic!()
    };
    assert!(
        request.config.is_none() && request.workspace.is_none() && request.runtime_root.is_none()
    );
    assert!(request.model.is_none() && request.session_name.is_none());
    assert_eq!(request.startup_session, StartupSession::Empty);
    let Command::Check { request, .. } = parse(&["config", "check"]).unwrap() else {
        panic!()
    };
    assert!(
        request.config.is_none()
            && request.workspace.is_none()
            && request.runtime_root.is_none()
            && request.model.is_none()
    );
    let session = "ses_eb278475-f606-7143-87df-8cb657e1c7ee";
    let node = "node_c346d387-9a21-70f0-8e5c-7422521183b3";
    let Command::Launch(request) = parse(&[
        "--session",
        session,
        "--node",
        node,
        "--name",
        "  display  ",
    ])
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        request.startup_session,
        StartupSession::Select {
            session: SessionId::new(session),
            node: Some(SessionNodeId::new(node))
        }
    );
    assert_eq!(request.session_name.as_deref(), Some("display"));
    let conversation = "conv_eb278475-f606-7143-87df-8cb657e1c7ee";
    assert!(parse(&["--inspect-conversation", conversation]).is_ok());
    for args in [
        vec!["--session", session],
        vec!["--node", node],
        vec!["--name", "x"],
    ] {
        assert!(parse(&[vec!["--inspect-conversation", conversation], args].concat()).is_err());
    }
}
#[test]
fn cli05_explicit_false_and_typed_init_values() {
    let base = [
        "init",
        "--template=anthropic",
        "--provider=local",
        "--endpoint=http://localhost",
        "--credential-env=KEY",
    ];
    let Command::Init { request, .. } = parse(
        &[
            base.to_vec(),
            vec!["--tool-calls", "false", "--reasoning=false", "--compat="],
        ]
        .concat(),
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(request.tool_calls, Some(false));
    assert_eq!(request.reasoning, Some(false));
    assert_eq!(request.compat.as_deref(), Some(""));
    assert!(
        request.model_id.is_none()
            && request.model_document.is_none()
            && request.context_window.is_none()
    );
    for args in [
        vec!["--tool-calls"],
        vec!["--tool-calls=yes"],
        vec!["--context-window=abc"],
        vec!["--max-output=4294967296"],
        vec!["--provider=other"],
    ] {
        assert!(parse(&[base.to_vec(), args].concat()).is_err());
    }
}
#[test]
fn cli06_equals_dash_values_and_terminator() {
    assert!(parse(&["config", "check", "--workspace", "--json"]).is_err());
    let Command::Check { request, json } =
        parse(&["config", "check", "--workspace=--json"]).unwrap()
    else {
        panic!()
    };
    assert!(!json);
    assert_eq!(request.workspace, Some("--json".into()));
    assert!(matches!(
        parse(&["config", "check", "--workspace=--json", "--json"]).unwrap(),
        Command::Check { json: true, .. }
    ));
    assert!(parse(&["--", "--model=x"]).is_err());
    assert!(parse(&["workflow", "check", "--", "review"]).is_ok());
    assert!(parse(&["config", "check", "--json=false"]).is_err());
}
#[test]
fn cli08_help_is_pure_and_cli09_child_is_not_public() {
    for args in [
        vec!["--help"],
        vec!["config", "--help"],
        vec!["config", "show", "--help"],
        vec!["workflow", "check", "--help"],
        vec!["init", "--help"],
        vec!["doctor", "--help"],
        vec!["app-server", "--help"],
        vec!["help", "workflow", "explain"],
    ] {
        let Command::Help(help) = parse(&args).unwrap() else {
            panic!("{args:?}")
        };
        assert!(help.contains("Usage:"));
        assert!(!help.contains("subagent-child") && !help.contains('\u{1b}'));
    }
    assert!(parse(&["--subagent-child"]).is_err());
}

#[test]
fn cli08_production_static_dispatch_has_zero_prohibited_effects() {
    let root = tempfile::tempdir().unwrap();
    let workspace = format!("--workspace={}", root.path().display());
    let config = format!("--config={}/missing.toml", root.path().display());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for args in [
        vec!["--help"],
        vec!["init", "--help"],
        vec!["app-server", "--help"],
        vec!["config", "check", &workspace, &config],
        vec!["config", "show", "--sources", &workspace, &config],
        vec!["config", "show", "--agent=main", &workspace, &config],
        vec!["workflow", "check", "review", &workspace, &config],
        vec!["workflow", "explain", "review", &workspace, &config],
    ] {
        let (_, effects) = super::super::static_effects::measure(|| {
            runtime.block_on(super::super::run_process(
                args.iter().map(ToString::to_string),
            ))
        });
        assert_eq!(effects, [0; 12], "{args:?}");
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn exact_paths_survive_public_cli_conversion() {
    for value in [" /tmp/rustx config ", "/tmp/rustx config ", " "] {
        for prefix in [vec![], vec!["config", "check"]] {
            let command = parse(
                &[
                    prefix,
                    vec![
                        "--config",
                        value,
                        "--workspace",
                        value,
                        "--runtime-root",
                        value,
                    ],
                ]
                .concat(),
            )
            .unwrap();
            let (Command::Launch(request) | Command::Check { request, .. }) = command else {
                panic!()
            };
            for path in [request.config, request.workspace, request.runtime_root] {
                assert_eq!(path.unwrap().as_os_str(), std::ffi::OsStr::new(value));
            }
        }
        let Command::Init { request, .. } = parse(&[
            "init",
            "--template=custom",
            "--provider=local",
            "--endpoint=http://localhost",
            "--credential-env=KEY",
            "--model-document",
            value,
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            request.model_document.unwrap().as_os_str(),
            std::ffi::OsStr::new(value)
        );
    }
}

#[test]
fn exact_init_strings_reach_native_credential_policy() {
    for value in [" RUSTX_TEST_KEY ", " "] {
        let Command::Init { request, .. } = parse(&[
            "init",
            "--template=anthropic",
            "--provider",
            value,
            "--endpoint",
            value,
            "--credential-env",
            value,
            "--model-id",
            value,
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(request.provider, value);
        assert_eq!(request.endpoint, value);
        assert_eq!(request.credential_env, value);
        assert_eq!(request.model_id.as_deref(), Some(value));
        assert!(
            super::super::initialization::documents(&request)
                .unwrap_err()
                .contains("--credential-env requires an environment variable name")
        );
    }
    let Command::Show { agent, .. } = parse(&["config", "show", "--agent", " main "]).unwrap()
    else {
        panic!()
    };
    assert_eq!(agent.as_deref(), Some(" main "));
}

#[test]
fn exact_empty_values_are_rejected_without_blanket_whitespace_rejection() {
    for args in [
        vec!["--config="],
        vec!["--workspace="],
        vec!["--runtime-root="],
        vec!["app-server", "--listen="],
        vec!["app-server", "--listen=stdio", "--token-file="],
    ] {
        assert!(parse(&args).is_err(), "{args:?}");
    }
    for flag in ["--provider", "--endpoint", "--credential-env"] {
        let mut args = vec![
            "init",
            "--template=custom",
            "--provider",
            "local",
            "--endpoint",
            "http://localhost",
            "--credential-env",
            "KEY",
        ];
        let index = args.iter().position(|arg| *arg == flag).unwrap();
        args[index + 1] = "";
        assert!(parse(&args).is_err());
    }
    assert!(
        parse(&[
            "init",
            "--template=custom",
            "--provider=local",
            "--endpoint=http://localhost",
            "--credential-env=KEY",
            "--model-document="
        ])
        .is_err()
    );
}

#[test]
fn launch_normalization_is_explicit_after_exact_lexical_parsing() {
    let session = " ses_eb278475-f606-7143-87df-8cb657e1c7ee ";
    let node = " node_c346d387-9a21-70f0-8e5c-7422521183b3 ";
    let args = [
        "--model",
        " local/model ",
        "--name",
        " display ",
        "--session",
        session,
        "--node",
        node,
    ];
    let lexical = Cli::try_parse_from(std::iter::once("rustx").chain(args)).unwrap();
    assert_eq!(
        lexical.launch.selection.model.as_deref(),
        Some(" local/model ")
    );
    assert_eq!(lexical.launch.name.as_deref(), Some(" display "));
    assert_eq!(lexical.launch.session.as_deref(), Some(session));
    assert_eq!(lexical.launch.node.as_deref(), Some(node));
    let Command::Launch(request) = parse(&args).unwrap() else {
        panic!()
    };
    assert_eq!(request.model.as_deref(), Some("local/model"));
    assert_eq!(request.session_name.as_deref(), Some("display"));
    assert_eq!(
        request.startup_session,
        StartupSession::Select {
            session: SessionId::new(session.trim()),
            node: Some(SessionNodeId::new(node.trim())),
        }
    );
    let conversation = " conv_eb278475-f606-7143-87df-8cb657e1c7ee ";
    let Command::Launch(request) = parse(&["--inspect-conversation", conversation]).unwrap() else {
        panic!()
    };
    assert_eq!(
        request.startup_session,
        StartupSession::InspectConversation {
            conversation_id: crate::runtime::identity::ConversationId::parse(conversation.trim())
                .unwrap(),
        }
    );
    for flag in ["--model", "--name", "--session", "--inspect-conversation"] {
        assert!(parse(&[flag, " "]).is_err());
    }
    let Command::Check { request, .. } =
        parse(&["config", "check", "--model", " local/model "]).unwrap()
    else {
        panic!()
    };
    assert_eq!(request.model.as_deref(), Some("local/model"));
}
