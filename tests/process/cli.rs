//! The public `rustx` process contract (#428).
//!
//! App Server is the only external product control protocol. A bare `rustx`
//! is help, and the retired standalone launch grammar is a bounded usage
//! error. Neither composes anything: with stdin held open, missing or invalid
//! authored configuration, a listening provider, and a selectable workspace
//! MCP server in place, the process exits on its own, writes nothing to
//! stdout, and leaves every byte under HOME and the workspace untouched.
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;

const LIVENESS: std::time::Duration = std::time::Duration::from_mins(1);
const SESSION: &str = "ses_eb278475-f606-7143-87df-8cb657e1c7ee";
const NODE: &str = "node_c346d387-9a21-70f0-8e5c-7422521183b3";
const CONVERSATION: &str = "conv_eb278475-f606-7143-87df-8cb657e1c7ee";

#[derive(Clone, Copy, Debug)]
enum Authored {
    Valid,
    Missing,
    Invalid,
}

struct Fixture {
    root: tempfile::TempDir,
    provider: std::net::TcpListener,
}

impl Fixture {
    fn new(authored: Authored) -> Self {
        let root = tempfile::tempdir().unwrap();
        let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        provider.set_nonblocking(true).unwrap();
        let user = root.path().join("home/rustx");
        std::fs::create_dir_all(&user).unwrap();
        match authored {
            Authored::Valid => std::fs::write(
                user.join("rustx.toml"),
                format!(
                    "{SESSION_TOML}\n{}",
                    models(&format!("http://{}/v1", provider.local_addr().unwrap()))
                ),
            )
            .unwrap(),
            Authored::Invalid => {
                std::fs::write(user.join("rustx.toml"), "agent = [not toml").unwrap();
            }
            Authored::Missing => {}
        }
        // A workspace whose selected MCP server would leave a sentinel file
        // if anything composed a runtime for it.
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".agents")).unwrap();
        std::fs::write(
            workspace.join(".agents/mcp.toml"),
            toml::to_string_pretty(&serde_json::json!({"mcp_servers": {"project": {
                "command": "touch",
                "args": [root.path().join("mcp-started")],
            }}}))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            workspace.join("rustx.toml"),
            "[agent.tools.sources]\nproject = \"all\"\n",
        )
        .unwrap();
        Self { root, provider }
    }

    fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    fn workspace(&self) -> PathBuf {
        self.root.path().join("workspace")
    }

    /// Runs the real binary with stdin held open for the whole run, so a
    /// process that waited for a protocol session could never exit here.
    async fn run(&self, arguments: &[String]) -> std::process::Output {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_rustx"))
            .current_dir(self.workspace())
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env(
                "RUSTX_PROCESS_TEST_KEY",
                "RUSTX_SECRET_SENTINEL_DO_NOT_LEAK",
            )
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let output = tokio::time::timeout(LIVENESS, child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("{arguments:?} waited on open stdin"))
            .unwrap();
        drop(stdin);
        output
    }

    fn assert_no_effects(&self, before: &Tree, arguments: &[String]) {
        assert_eq!(
            &tree(self.root.path()),
            before,
            "{arguments:?} changed state"
        );
        assert!(!self.root.path().join("mcp-started").exists());
        assert_eq!(
            self.provider.accept().unwrap_err().kind(),
            ErrorKind::WouldBlock,
            "{arguments:?} contacted the provider"
        );
    }
}

type Tree = std::collections::BTreeMap<PathBuf, Option<Vec<u8>>>;

fn tree(root: &Path) -> Tree {
    let mut tree = Tree::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            tree.insert(path.clone(), None);
            tree.extend(self::tree(&path));
        } else {
            tree.insert(path.clone(), Some(std::fs::read(path).unwrap()));
        }
    }
    tree
}

fn args(arguments: &[&str]) -> Vec<String> {
    arguments.iter().map(ToString::to_string).collect()
}

#[tokio::test]
async fn bare_and_help_invocations_print_help_on_stderr_without_effects() {
    for authored in [Authored::Valid, Authored::Missing, Authored::Invalid] {
        let f = Fixture::new(authored);
        for arguments in [args(&[]), args(&["--help"]), args(&["help"])] {
            let before = tree(f.root.path());
            let output = f.run(&arguments).await;
            assert_eq!(output.status.code(), Some(0), "{authored:?} {arguments:?}");
            assert!(output.stdout.is_empty(), "{arguments:?} wrote stdout");
            let help = String::from_utf8(output.stderr).unwrap();
            assert!(help.contains("Usage: rustx") && help.contains("app-server"));
            assert!(!help.contains("subagent-child") && !help.contains("--session"));
            f.assert_no_effects(&before, &arguments);
        }
    }
}

#[tokio::test]
async fn retired_launch_invocations_are_bounded_usage_errors_without_effects() {
    let workspace = |f: &Fixture| f.workspace().display().to_string();
    for authored in [Authored::Valid, Authored::Missing, Authored::Invalid] {
        let f = Fixture::new(authored);
        let config = f.home().join("rustx/rustx.toml").display().to_string();
        let runtime = f.root.path().join("runtime").display().to_string();
        for arguments in [
            args(&["--session", SESSION]),
            args(&["--session", SESSION, "--node", NODE]),
            args(&["--node", NODE]),
            args(&["--name", "display"]),
            args(&["--inspect-conversation", CONVERSATION]),
            args(&["--model", "fixture/process-model"]),
            args(&["--config", &config]),
            args(&["--workspace", &workspace(&f)]),
            args(&["--runtime-root", &runtime]),
            args(&[
                "--config",
                &config,
                "--workspace",
                &workspace(&f),
                "--runtime-root",
                &runtime,
            ]),
            args(&[
                "--runtime-root",
                &runtime,
                "app-server",
                "--listen",
                "stdio",
            ]),
            args(&["--models"]),
            args(&["--future", "x"]),
            args(&["--subagent-child", "--session", SESSION]),
        ] {
            let before = tree(f.root.path());
            let output = f.run(&arguments).await;
            assert_eq!(output.status.code(), Some(2), "{authored:?} {arguments:?}");
            assert!(output.stdout.is_empty(), "{arguments:?} wrote stdout");
            let diagnostic = String::from_utf8(output.stderr).unwrap();
            assert!(
                diagnostic.len() < 1024 && !diagnostic.contains("RUSTX_SECRET_SENTINEL"),
                "{arguments:?}: {diagnostic}"
            );
            if arguments[0] != "--subagent-child" {
                assert!(
                    diagnostic.contains("Usage: rustx"),
                    "{arguments:?}: {diagnostic}"
                );
            }
            f.assert_no_effects(&before, &arguments);
        }
    }
}

/// The same option spellings stay valid under the surviving explicit
/// subcommands, and the effect detector above is not vacuous: a real App
/// Server startup visibly binds runtime storage in the same fixture.
#[tokio::test]
async fn surviving_commands_keep_their_options_and_startup_effects_are_observable() {
    let f = Fixture::new(Authored::Valid);
    let config = f.home().join("rustx/rustx.toml").display().to_string();
    let runtime = f.root.path().join("runtime").display().to_string();
    let workspace = f.workspace().display().to_string();
    let arguments = args(&[
        "config",
        "check",
        "--json",
        "--config",
        &config,
        "--workspace",
        &workspace,
        "--runtime-root",
        &runtime,
        "--model",
        "fixture/process-model",
    ]);
    let before = tree(f.root.path());
    let output = f.run(&arguments).await;
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["operation"], "config_check");
    f.assert_no_effects(&before, &arguments);

    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_rustx"))
        .current_dir(f.workspace())
        .env_clear()
        .env("HOME", f.home())
        .args([
            "app-server",
            "--listen",
            "stdio",
            "--runtime-root",
            &runtime,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = server.stdin.take().unwrap();
    let mut lines =
        tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(server.stdout.take().unwrap()));
    tokio::io::AsyncWriteExt::write_all(&mut input, format!("{INITIALIZE}\n").as_bytes())
        .await
        .unwrap();
    let reply: serde_json::Value = serde_json::from_str(
        &tokio::time::timeout(LIVENESS, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("the first stdout record is the initialize response"),
    )
    .unwrap();
    assert_eq!(
        reply["id"], 1,
        "App Server stdout starts with protocol, no banner"
    );
    assert_ne!(
        tree(f.root.path()),
        before,
        "startup effects are observable"
    );
    server.kill().await.unwrap();
}

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol_version":33,"client":{"name":"cli-contract","version":"1"},"presentation":{"images":false,"questionnaires":false,"reviews":false}}}"#;

fn models(base_url: &str) -> String {
    format!(
        r#"[providers.fixture]
base_url = "{base_url}"
api_key = "$RUSTX_PROCESS_TEST_KEY"

[models."fixture/process-model"]
provider = "fixture"
id = "process-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[models."fixture/process-model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models."fixture/process-model".compat]
chat_reasoning_replay = "omit"
"#
    )
}

const SESSION_TOML: &str = r#"agent_id = "agent-process"

[agent]
[agent.model]
model = "fixture/process-model"
"#;
