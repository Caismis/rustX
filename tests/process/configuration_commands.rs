//! Real binary command grammar, output framing, exit and zero-state-write contract.
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, arguments: &[impl AsRef<std::ffi::OsStr>]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rustx"))
        .current_dir(root.join("workspace"))
        .env_clear()
        .env("HOME", root.join("home"))
        .env("RUSTX_TEST_KEY", "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK")
        .args(arguments)
        .output()
        .expect("Rust configuration command")
}

fn report(output: &Output, exit: i32) -> serde_json::Value {
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("RUSTX_SECRET_SENTINEL_DO_NOT_LEAK"));
    }
    serde_json::from_slice(&output.stdout).expect("one structured JSON record")
}

fn state_tree(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    let mut tree = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            tree.insert(path.clone(), None);
            tree.extend(state_tree(&path));
        } else {
            tree.insert(path.clone(), Some(std::fs::read(path).unwrap()));
        }
    }
    tree
}

#[test]
fn cfg237_binary_workflow_commands_share_json_exit_and_read_only_contract() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let user = root.path().join("home/rustx");
    let workflows = workspace.join(".agents/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("rustx.toml"),
        include_bytes!("../../examples/local-runtime/minimal/rustx.toml"),
    )
    .unwrap();
    std::fs::write(
        workspace.join("rustx.toml"),
        r"[agent]
workflows = ['human_plan']
",
    )
    .unwrap();
    let file = workflows.join("human_plan.yaml");
    let valid = include_bytes!(
        "../../examples/local-runtime/workflow-templates/.agents/workflows/human_plan.yaml"
    );
    std::fs::write(&file, valid).unwrap();
    let state = root.path().join("home");
    let before = state_tree(&state);
    for operation in ["check", "explain"] {
        let value = report(
            &run(
                root.path(),
                &["workflow", operation, "human_plan", "--json"],
            ),
            3,
        );
        assert_eq!(value["validity"], "valid");
        assert_eq!(value["readiness"], "unresolved");
        assert_eq!(value["workflow"]["admission"]["status"], "enabled");
        assert_eq!(
            value["workflow"]["program"].is_object(),
            operation == "explain"
        );
        assert!(
            run(root.path(), &["workflow", operation, "human_plan"])
                .stdout
                .starts_with(format!("workflow_{operation}:").as_bytes())
        );
        lexical_failure(&run(
            root.path(),
            &[
                "workflow",
                operation,
                "human_plan",
                "--trust",
                "grant",
                "--json",
            ],
        ));
        assert_eq!(std::fs::read(&file).unwrap(), valid);
    }
    std::fs::write(&file, "description: [\n").unwrap();
    for operation in ["check", "explain"] {
        let invalid = report(
            &run(
                root.path(),
                &["workflow", operation, "human_plan", "--json"],
            ),
            2,
        );
        assert_eq!(invalid["validity"], "invalid");
        assert!(invalid["diagnostics"][0]["line"].is_number());
    }
    assert_eq!(state_tree(&state), before);
    assert!(!workspace.join(".git").exists());
}

#[test]
fn cfg235_binary_init_check_show_exit_and_machine_contract() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let incomplete = report(
        &run(root.path(), &["config", "show", "--sources", "--json"]),
        3,
    );
    assert_eq!(incomplete["validity"], "incomplete");
    assert!(incomplete["launch"].is_null());
    assert!(incomplete["partial"].is_object());
    lexical_failure(&run(root.path(), &["config", "show", "--json"]));
    let arguments = [
        "init",
        "--template",
        "openai-chat",
        "--provider",
        "local",
        "--model-id",
        "declared",
        "--endpoint",
        "http://127.0.0.1:9/v1",
        "--credential-env",
        "RUSTX_TEST_KEY",
        "--context-window",
        "128000",
        "--max-output",
        "4096",
        "--tool-calls",
        "true",
        "--reasoning",
        "false",
        "--compat",
        "chat_reasoning_replay = \"omit\"",
        "--json",
    ];
    let initialized = report(&run(root.path(), &arguments), 0);
    assert!(initialized["readiness"].is_null());
    assert_eq!(
        initialized["initialization"]["written"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let models = root.path().join("home/rustx/rustx.toml");
    let before = std::fs::read(&models).unwrap();
    assert!(String::from_utf8_lossy(&before).contains("$RUSTX_TEST_KEY"));
    report(&run(root.path(), &arguments), 2);
    assert_eq!(std::fs::read(models).unwrap(), before);
    assert!(!root.path().join("home/.local/state").exists());
    assert!(!root.path().join("workspace/rustx.toml").exists());
    let prospective = report(&run(root.path(), &["config", "check", "--json"]), 3);
    assert_eq!(prospective["validity"], "valid");
    assert!(prospective["launch"].get("trusted").is_none());
    assert!(!root.path().join("home/.local/state").exists());
    let checked = report(&run(root.path(), &["config", "check", "--json"]), 3);
    let shown = report(
        &run(root.path(), &["config", "show", "--sources", "--json"]),
        3,
    );
    assert_eq!(checked["launch"], shown["launch"]);
    assert_eq!(shown["validity"], "valid");
    assert_eq!(shown["readiness"], "unresolved");
    assert_eq!(shown["scope"], "prospective_next_launch");
    assert_eq!(shown["launch"]["selected_model"], "local/declared");
    let runtime_root = shown["launch"]["runtime_root"].as_str().unwrap();
    assert!(!Path::new(runtime_root).exists());
    std::fs::write(
        root.path().join("workspace/rustx.toml"),
        "unknown_field = true",
    )
    .unwrap();
    let invalid = report(&run(root.path(), &["config", "check", "--json"]), 2);
    assert_eq!(invalid["diagnostics"][0]["path"], "$");
    assert!(invalid["diagnostics"][0]["file"].is_string());
    assert!(!Path::new(runtime_root).exists());
    let help = run(root.path(), &["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stderr).contains("3 incomplete or unresolved"));
}

#[test]
fn cfg235_binary_doctor_discloses_plan_and_preserves_mixed_results() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let user = root.path().join("home/rustx");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("rustx.toml"),
        include_bytes!("../../examples/local-runtime/minimal/rustx.toml"),
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("workspace/.agents")).unwrap();
    std::fs::write(
        root.path().join("workspace/.agents/mcp.toml"),
        "[mcp_servers.unselected]\ncommand = \"must-never-spawn\"\n",
    )
    .unwrap();
    let output = run(root.path(), &["doctor", "--probe", "--json"]);
    assert_eq!(output.status.code(), Some(3));
    let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["phase"], "probe_plan");
    assert_eq!(records[1]["phase"], "probe_results");
    for target in records[0]["targets"].as_array().unwrap() {
        for effect in [
            "spawn_process",
            "network",
            "prepare_environment",
            "resolve_credentials",
        ] {
            assert_eq!(target[effect], false);
        }
    }
    assert_eq!(records[1]["results"][0]["state"], "unresolved");
    assert_eq!(
        records[1]["results"].as_array().unwrap().len(),
        2,
        "discovered sources remain inert without admitted demand"
    );
    assert!(!root.path().join("home/.local/state").exists());

    std::fs::write(root.path().join("workspace/rustx.toml"), "").unwrap();
    std::fs::create_dir_all(root.path().join("workspace/.agents/tools/unprepared")).unwrap();
    let output = run(root.path(), &["doctor", "--probe", "--prepare", "--json"]);
    assert_eq!(output.status.code(), Some(3));
    let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[1]["results"][1]["state"], "skipped");
    assert_eq!(records[0]["targets"][1]["prepare_environment"], false);
    assert!(!root.path().join("home/rustx/runtime").exists());
}

#[test]
fn cfg275_agent_inspection_and_removed_flags_are_offline() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let user = root.path().join("home/rustx");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("rustx.toml"),
        include_bytes!("../../examples/local-runtime/minimal/rustx.toml"),
    )
    .unwrap();
    let before = state_tree(&root.path().join("home"));
    let result = run(
        root.path(),
        &["config", "show", "--agent", "main", "--json"],
    );
    let value = report(&result, 3);
    assert_eq!(value["agent"]["identity"]["kind"], "main");
    assert!(value["agent"]["diagnostics"].is_array());
    for args in [
        vec!["config", "show"],
        vec!["config", "show", "--agent"],
        vec!["config", "show", "--sources", "--agent", "main"],
        vec!["config", "show", "--agent", "main", "--sources"],
    ] {
        let output = run(root.path(), &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(
            output.stdout.is_empty(),
            "invalid modes produce no inspection"
        );
    }
    for flag in ["--no-tools", "--no-skills"] {
        assert_eq!(
            run(
                root.path(),
                &["config", "show", "--agent", "main", flag, "--json"]
            )
            .status
            .code(),
            Some(2)
        );
    }
    assert_eq!(before, state_tree(&root.path().join("home")));
}

fn lexical_failure(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    assert!(!output.stderr.is_empty());
    assert!(!output.stderr.contains(&0x1b));
}

#[test]
fn cli01_cli02_cli06_cli07_public_entry_stream_matrix() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    for args in [
        vec!["--model"],
        vec!["--model="],
        vec!["--model=a", "--model=b"],
        vec!["--unknown"],
        vec!["--", "--model=x"],
        vec!["config", "check", "--workspace", "--json"],
        vec!["config", "check", "--unknown", "--json"],
        vec!["config", "check", "--json", "--unknown"],
        vec!["config", "unknown", "--json"],
        vec!["config", "show", "--agent", "--json"],
        vec!["doctor", "--prepare", "--json"],
        vec!["workflow", "check", "--json"],
        vec!["workflow", "check", "review", "--runtime-root=x", "--json"],
        vec!["init", "--json"],
        vec!["init", "--template=unknown", "--json"],
        vec!["app-server", "--listen"],
        vec!["app-server", "--listen=stdio", "--workspace=x"],
    ] {
        lexical_failure(&run(root.path(), &args));
    }
    let human = run(root.path(), &["config", "check", "--workspace=--json"]);
    assert_eq!(human.status.code(), Some(2));
    assert!(human.stderr.is_empty());
    assert!(human.stdout.starts_with(b"config_check:"));
    report(
        &run(
            root.path(),
            &["config", "check", "--workspace=--json", "--json"],
        ),
        2,
    );
    for args in [
        vec!["--config=/nonexistent/rustx.toml"],
        vec![
            "app-server",
            "--listen=stdio",
            "--config=/nonexistent/rustx.toml",
        ],
        vec!["app-server", "--listen=invalid"],
        vec!["app-server", "--listen=stdio", "--token-file=x"],
    ] {
        lexical_failure(&run(root.path(), &args));
    }
    assert!(!root.path().join("home").exists());
}

#[test]
fn cli07_cli08_help_precedes_host_capture_and_has_no_effects() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["--help"],
        vec!["-h"],
        vec!["config", "--help"],
        vec!["config", "check", "--help"],
        vec!["config", "show", "--help"],
        vec!["workflow", "--help"],
        vec!["workflow", "check", "--help"],
        vec!["workflow", "explain", "--help"],
        vec!["doctor", "--help"],
        vec!["init", "--help"],
        vec!["app-server", "--help"],
        vec!["help", "config", "show"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rustx"))
            .current_dir(root.path())
            .env_clear()
            .env("HOME", "relative-invalid-home")
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {:?}",
            output.stderr
        );
        assert!(output.stdout.is_empty());
        let help = String::from_utf8(output.stderr).unwrap();
        assert!(
            help.contains("Usage:") && !help.contains("subagent-child") && !help.contains('\u{1b}')
        );
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn cli09_internal_child_rejects_all_extra_arguments() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    for args in [
        vec!["--subagent-child", "--help"],
        vec!["--subagent-child", "--model=x"],
        vec!["app-server", "--subagent-child"],
        vec!["--subagent-child", "--subagent-child"],
    ] {
        let output = run(root.path(), &args);
        lexical_failure(&output);
        assert!(String::from_utf8_lossy(&output.stderr).contains("internal mode"));
    }
    let output = run(root.path(), &["--subagent-child"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    assert!(!root.path().join("home").exists());
}

#[test]
fn exact_values_reach_native_process_owners() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    // The trimmed path names an invalid document; only the exact path is valid.
    let exact = workspace.join("settings.toml ");
    std::fs::write(workspace.join("settings.toml"), "unknown_field = true").unwrap();
    std::fs::write(
        &exact,
        include_bytes!("../../examples/local-runtime/minimal/rustx.toml"),
    )
    .unwrap();
    let checked = report(
        &run(
            root.path(),
            &[
                "config",
                "check",
                "--config",
                exact.to_str().unwrap(),
                "--json",
            ],
        ),
        3,
    );
    assert_eq!(checked["validity"], "valid");

    let invalid = report(
        &run(
            root.path(),
            &[
                "init",
                "--template=anthropic",
                "--provider=local",
                "--endpoint=http://localhost",
                "--credential-env",
                " RUSTX_TEST_KEY ",
                "--json",
            ],
        ),
        2,
    );
    assert!(
        invalid["diagnostics"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("--credential-env requires an environment variable name")
    );

    // The token also keeps a regressed normalized stdio invocation finite:
    // it would fail with the *different* native stdio/token diagnostic.
    let transport = run(
        root.path(),
        &["app-server", "--listen", " stdio ", "--token-file=/unused"],
    );
    lexical_failure(&transport);
    assert!(
        String::from_utf8_lossy(&transport.stderr).contains("listen must be stdio or ws://IP:PORT")
    );
    assert!(!root.path().join("home").exists());
}

// Materializing arbitrary filename bytes is a Linux filesystem fixture contract.
#[cfg(target_os = "linux")]
#[test]
fn os_argv_non_unicode_model_document_selects_exact_file() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let exact = workspace.join(OsString::from_vec(b"model-\xff.toml".to_vec()));
    let lossy = std::path::PathBuf::from(exact.to_string_lossy().into_owned());
    assert_ne!(exact, lossy);
    std::fs::write(&lossy, "unknown_field = true").unwrap();
    let fixture: toml::Value = toml::from_str(include_str!(
        "../../examples/local-runtime/minimal/rustx.toml"
    ))
    .unwrap();
    std::fs::write(
        &exact,
        toml::to_string(&fixture["models"]["example/demo-model"]).unwrap(),
    )
    .unwrap();
    let before = state_tree(&workspace);
    let output = run(
        root.path(),
        &[
            OsString::from("init"),
            OsString::from("--template=custom"),
            OsString::from("--provider=example"),
            OsString::from("--endpoint=http://localhost"),
            OsString::from("--credential-env=RUSTX_TEST_KEY"),
            OsString::from("--model-document"),
            exact.into_os_string(),
            OsString::from("--json"),
        ],
    );
    let initialized = report(&output, 0);
    assert_eq!(
        initialized["initialization"]["written"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let published: toml::Value = toml::from_str(
        &std::fs::read_to_string(root.path().join("home/rustx/rustx.toml")).unwrap(),
    )
    .unwrap();
    for (field, expected) in fixture["models"]["example/demo-model"].as_table().unwrap() {
        assert_eq!(&published["models"]["example/demo-model"][field], expected);
    }
    assert_eq!(state_tree(&workspace), before);
    assert!(!root.path().join("home/.local/state").exists());
}

#[cfg(unix)]
#[test]
fn os_argv_non_unicode_text_is_a_lexical_failure() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let before = state_tree(root.path());
    let output = run(
        root.path(),
        &[
            OsString::from("app-server"),
            OsString::from("--listen"),
            OsString::from_vec(b"stdio-\xff".to_vec()),
        ],
    );
    lexical_failure(&output);
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("invalid UTF-8"), "{error}");
    assert!(!error.contains("panicked"));
    assert_eq!(state_tree(root.path()), before);
}

#[cfg(unix)]
#[test]
fn os_argv_private_child_discriminator_is_exact() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let before = state_tree(root.path());
    let child = OsString::from("--subagent-child");
    // Without an inherited control socket, the exact mode reaches its native
    // startup failure. It must not reach public clap parsing.
    let output = run(root.path(), std::slice::from_ref(&child));
    lexical_failure(&output);
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("subagent child:")
    );
    let invalid = OsString::from_vec(vec![0xff]);
    for args in [vec![child.clone(), invalid.clone()], vec![invalid, child]] {
        let output = run(root.path(), &args);
        lexical_failure(&output);
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("internal mode")
        );
    }
    let output = run(root.path(), &[OsString::from("--help")]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(
        !String::from_utf8(output.stderr)
            .unwrap()
            .contains("subagent-child")
    );
    assert_eq!(state_tree(root.path()), before);
}

// Materializing arbitrary filename bytes is a Linux filesystem fixture contract.
#[cfg(target_os = "linux")]
#[test]
fn os_argv_non_unicode_config_selects_exact_file() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let exact = workspace.join(OsString::from_vec(b"settings-\xff.toml".to_vec()));
    let lossy = std::path::PathBuf::from(exact.to_string_lossy().into_owned());
    assert_ne!(exact, lossy);
    std::fs::write(&lossy, "unknown_field = true").unwrap();
    std::fs::write(
        &exact,
        include_bytes!("../../examples/local-runtime/minimal/rustx.toml"),
    )
    .unwrap();
    let before = state_tree(root.path());
    let output = run(
        root.path(),
        &[
            OsString::from("config"),
            OsString::from("check"),
            OsString::from("--config"),
            exact.into_os_string(),
            OsString::from("--json"),
        ],
    );
    let checked = report(&output, 3);
    assert_eq!(checked["validity"], "valid");
    assert_eq!(checked["readiness"], "unresolved");
    assert_eq!(checked["projection_omitted"], true);
    assert!(
        checked["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["category"] == "projection_encoding")
    );
    let sibling = report(
        &run(
            root.path(),
            &[
                "config",
                "check",
                "--config",
                lossy.to_str().unwrap(),
                "--json",
            ],
        ),
        2,
    );
    assert_eq!(sibling["validity"], "invalid");
    assert_eq!(state_tree(root.path()), before);
}

#[cfg(unix)]
#[test]
fn os_argv_non_unicode_missing_config_reaches_native_owner() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    // No invalid-byte filename is created. Unix argv can carry the value even
    // when the host filesystem cannot materialize it (including macOS).
    let missing = workspace.join(OsString::from_vec(b"missing-\xff.toml".to_vec()));
    let before = state_tree(root.path());
    let output = run(
        root.path(),
        &[
            OsString::from("config"),
            OsString::from("check"),
            OsString::from("--config"),
            missing.into_os_string(),
            OsString::from("--json"),
        ],
    );
    // The filesystem may reject this spelling or report an absent optional
    // source. Both are native diagnostic outcomes, never a clap Unicode error.
    let code = output.status.code().unwrap();
    assert!(matches!(code, 2 | 3));
    let checked = report(&output, code);
    assert_eq!(checked["operation"], "config_check");
    assert_eq!(
        checked["validity"],
        if code == 2 { "invalid" } else { "incomplete" }
    );
    let diagnostics = checked["diagnostics"].as_array().unwrap();
    assert!(diagnostics.iter().any(|d| {
        d["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
            && d["category"] != "projection_encoding"
            && d["classification"] == if code == 2 { "error" } else { "warning" }
    }));
    let encoding_warning = diagnostics
        .iter()
        .any(|d| d["category"] == "projection_encoding");
    if code == 3 {
        // Resolution retained the non-Unicode source in its partial projection.
        assert!(encoding_warning);
        assert_eq!(checked["projection_omitted"], true);
    }
    if encoding_warning {
        assert_eq!(checked["projection_omitted"], true);
        assert!(checked["launch"].is_null() && checked["partial"].is_null());
    }
    assert_eq!(state_tree(root.path()), before);
}
