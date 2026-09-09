//! Real binary command grammar, output framing, exit and zero-state-write contract.
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, arguments: &[&str]) -> Output {
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

#[test]
fn cfg237_binary_workflow_commands_share_json_exit_and_read_only_contract() {
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
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let user = root.path().join("home/.config/rustx");
    let workflows = workspace.join(".agents/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("models.jsonc"),
        include_bytes!("../../examples/local-runtime/minimal/models.jsonc"),
    )
    .unwrap();
    std::fs::write(
        user.join("settings.jsonc"),
        include_bytes!("../../examples/local-runtime/minimal/settings.jsonc"),
    )
    .unwrap();
    std::fs::write(
        workspace.join("rustx.jsonc"),
        r#"{"workflows":{"definitions":["human_plan"],"main":[]}}"#,
    )
    .unwrap();
    let file = workflows.join("human_plan.yaml");
    let valid = include_bytes!(
        "../../examples/local-runtime/workflow-templates/.agents/workflows/human_plan.yaml"
    );
    std::fs::write(&file, valid).unwrap();
    assert!(run(root.path(), &["--trust", "grant"]).status.success());
    let state = root.path().join("home/.local/state");
    let before = state_tree(&state); // The explicit trust setup already wrote its membership directory.
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
        assert_eq!(value["workflow"]["registered"], true);
        assert_eq!(value["workflow"]["configured_main_admission"], false);
        assert_eq!(
            value["workflow"]["program"].is_object(),
            operation == "explain"
        );
        assert!(
            run(root.path(), &["workflow", operation, "human_plan"])
                .stdout
                .starts_with(format!("workflow_{operation}:").as_bytes())
        );
        report(
            &run(
                root.path(),
                &[
                    "workflow",
                    operation,
                    "human_plan",
                    "--trust",
                    "grant",
                    "--json",
                ],
            ),
            2,
        );
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
    let arguments_error = report(&run(root.path(), &["config", "show", "--json"]), 2);
    assert_eq!(arguments_error["diagnostics"][0]["path"], "arguments");
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
        "{\"chatReasoningReplay\":\"omit\"}",
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
    let models = root.path().join("home/.config/rustx/models.jsonc");
    let before = std::fs::read(&models).unwrap();
    assert!(String::from_utf8_lossy(&before).contains("$RUSTX_TEST_KEY"));
    report(&run(root.path(), &arguments), 2);
    assert_eq!(std::fs::read(models).unwrap(), before);
    assert!(!root.path().join("home/.local/state").exists());
    assert!(!root.path().join("workspace/rustx.jsonc").exists());
    let untrusted = report(&run(root.path(), &["config", "check", "--json"]), 3);
    assert_eq!(untrusted["validity"], "valid");
    assert_eq!(untrusted["launch"]["trusted"], false);
    assert!(!root.path().join("home/.local/state").exists());
    assert!(run(root.path(), &["--trust", "grant"]).status.success());
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
        root.path().join("workspace/rustx.jsonc"),
        "{\"unknownField\":true}",
    )
    .unwrap();
    let invalid = report(&run(root.path(), &["config", "check", "--json"]), 2);
    assert_eq!(invalid["diagnostics"][0]["path"], "unknownField");
    assert!(invalid["diagnostics"][0]["file"].is_string());
    assert!(!Path::new(runtime_root).exists());
    let help = run(root.path(), &["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("3 incomplete or unresolved"));
}

#[test]
fn cfg235_binary_doctor_discloses_plan_and_preserves_mixed_results() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("workspace")).unwrap();
    let user = root.path().join("home/.config/rustx");
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(
        user.join("models.jsonc"),
        include_str!("../../examples/local-runtime/minimal/models.jsonc"),
    )
    .unwrap();
    std::fs::write(
        user.join("settings.jsonc"),
        include_str!("../../examples/local-runtime/minimal/settings.jsonc"),
    )
    .unwrap();
    std::fs::write(
        root.path().join("workspace/rustx.jsonc"),
        r#"{"mcpServers":{"disabled":{"enabled":false,"command":"must-never-spawn"}}}"#,
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
    assert_eq!(records[1]["results"][1]["state"], "skipped");
    assert!(!root.path().join("home/.local/state").exists());

    assert!(run(root.path(), &["--trust", "grant"]).status.success());
    std::fs::write(
        root.path().join("workspace/rustx.jsonc"),
        r#"{"pythonSources":{"python:missing":"enabled"}}"#,
    )
    .unwrap();
    let output = run(root.path(), &["doctor", "--probe", "--prepare", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let records: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[1]["results"][0]["state"], "unresolved");
    assert_eq!(records[1]["results"][1]["state"], "unavailable");
    assert_eq!(records[0]["targets"][1]["prepare_environment"], false);
    assert!(!root.path().join("workspace/.rustx").exists());
}
