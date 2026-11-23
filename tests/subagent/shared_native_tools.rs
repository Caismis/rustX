//! Real child native mutations against a nested, frozen Session binding.
use super::*;
use serde_json::{Value, json};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const SHARED_MOUNT_WORKFLOW: &str = r"description: Read the bound Workspace through the Workflow tool plane.
timeout_ms: 10000
block:
  input: {type: object, properties: {}, additionalProperties: false}
  nodes:
    workflow_start: {type: start}
    inspect:
      type: tool
      selector: {origin: builtin, name: read}
      arguments: {type: literal, value: {path: workflow.txt}}
      result: {type: text}
    done:
      type: return
      outputs: [{variable: observed, value_selector: [inspect, text]}]
  edges: [{from: workflow_start, to: inspect}, {from: inspect, to: done}]
";

fn tool_reply(name: &str, arguments: &Value) -> crate::common::FixtureReply {
    let arguments = crate::common::tool_call_envelope(arguments.clone());
    let chunk = json!({"id":"shared-tools", "object":"chat.completion.chunk",
        "created":1, "model":"subagent-model", "choices":[{"index":0,
        "delta":{"role":"assistant", "tool_calls":[{"index":0,
        "id":format!("shared-{name}"), "type":"function",
        "function":{"name":name,"arguments":arguments.to_string()}}]},
        "finish_reason":"tool_calls"}]});
    crate::common::FixtureReply::body(
        200,
        "OK",
        "text/event-stream",
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
    )
}

fn child_results(request: &Value) -> Vec<&Value> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect()
}

fn route_native_tools(body: &str) -> crate::common::FixtureReply {
    let request: Value = serde_json::from_str(body).unwrap();
    let results = child_results(&request);
    if body.contains("ROOT-NATIVE-TOOLS") {
        return if results.is_empty() {
            tool_reply(
                "subagent",
                &json!({"title":"Exercise native file tools", "agent":"explore", "task":"CHILD-NATIVE-TOOLS"}),
            )
        } else {
            assert!(
                (1..=3).contains(&results.len()),
                "only the three admitted Root calls"
            );
            assert_eq!(results[0]["tool_call_id"], "shared-subagent");
            let receipt: Value = serde_json::from_str(
                results[0]["content"]
                    .as_str()
                    .expect("Chat tool content is plain text"),
            )
            .expect("the creation result retains its exact business JSON");
            assert_eq!(receipt.as_object().unwrap().len(), 4);
            assert_eq!(receipt["agent"], "explore");
            assert_eq!(receipt["state"], "active");
            for identity in ["agent_id", "activation_id"] {
                assert!(receipt[identity].as_str().is_some_and(|id| !id.is_empty()));
            }
            match results.as_slice() {
                [_] => tool_reply("read", &json!({"path":"root.txt"})),
                [_, root] => {
                    assert_eq!(root["tool_call_id"], "shared-read");
                    assert!(
                        root["content"]
                            .as_str()
                            .unwrap()
                            .contains("client-root-read")
                    );
                    tool_reply("inspect_shared_mount", &json!({}))
                }
                [_, _, workflow] => {
                    assert_eq!(workflow["tool_call_id"], "shared-inspect_shared_mount");
                    assert!(
                        workflow["content"]
                            .as_str()
                            .unwrap()
                            .contains("client-workflow-read")
                    );
                    crate::common::sse_fixture("openai_chat", "plain_text.sse")
                }
                _ => unreachable!("root tool count validated"),
            }
        };
    }
    assert!(body.contains("CHILD-NATIVE-TOOLS"));
    let mut tools: Vec<_> = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    tools.sort_unstable();
    assert_eq!(
        tools,
        ["bash", "edit", "read", "write"],
        "child scope has no Root controls"
    );
    // Advance only from the actual committed Tool result, never request count
    // or wall time. Every mutation is performed by the real child runtime.
    match results.len() {
        0 => tool_reply("write", &json!({"path":"child.txt", "content":"before\n"})),
        1 => {
            let written = results[0]["content"]
                .as_str()
                .expect("Chat tool content is plain text");
            assert!(
                written.contains("Successfully wrote"),
                "child Write: {written}"
            );
            tool_reply(
                "edit",
                &json!({"path":"child.txt", "edits":[{"oldText":"before", "newText":"after"}]}),
            )
        }
        2 => {
            assert!(
                results[1]["content"]
                    .as_str()
                    .expect("Chat tool content is plain text")
                    .contains("Successfully replaced")
            );
            tool_reply("read", &json!({"path":"child.txt"}))
        }
        3 => {
            assert!(
                results[2]["content"]
                    .as_str()
                    .expect("Chat tool content is plain text")
                    .contains("after")
            );
            tool_reply(
                "bash",
                &json!({"command":"read -r ready < .python-probe-ready && test \"$(cat child.txt)\" = after && printf 'NATIVE-BASH-OK\\n'", "execution_mode":"foreground"}),
            )
        }
        4 => {
            assert!(
                results[3]["content"]
                    .as_str()
                    .expect("Chat tool content is plain text")
                    .contains("NATIVE-BASH-OK")
            );
            crate::common::sse_fixture("openai_chat", "subagent_child_answer.sse")
        }
        count => panic!("unexpected child Tool result count: {count}"),
    }
}

fn git(repository: &std::path::Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {:?}", output.stderr);
    output.stdout
}

fn stage_parent_with_client_path(
    root: &std::path::Path,
    home: &std::path::Path,
    path: &str,
    mount: Option<(&std::path::Path, &std::path::Path)>,
) -> std::path::PathBuf {
    let program = std::path::Path::new(env!("CARGO_BIN_EXE_rustx"));
    let binaries = program.parent().unwrap();
    let staged = root.join("bin");
    std::fs::create_dir(&staged).unwrap();
    for name in [
        "rustx",
        "bash-supervisor",
        "interactive-supervisor",
        "workflow-renderer",
    ] {
        let source = binaries.join(name);
        let target = staged.join(name);
        if std::fs::hard_link(&source, &target).is_err() {
            std::fs::copy(&source, &target).unwrap();
        }
    }
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let scratch = staged.join("tmp");
    std::fs::create_dir(&scratch).unwrap();
    let client = quote(binaries.join("rustx-execution-client").to_str().unwrap());
    let home = quote(home.to_str().unwrap());
    let scratch = quote(scratch.to_str().unwrap());
    let command = mount.map_or_else(
        || format!("exec {client}"),
        |(physical, logical)| {
            // The extra mount wrapper has its own PID. This fixture uses
            // the Client's private staging inside that mount.
            format!(
                "unset RUSTX_CLIENT_DOMAIN_STAGING\nexec bwrap --ro-bind / / --dev /dev --bind {} {} --bind {home} {home} --tmpfs {scratch} -- {client}",
                quote(physical.to_str().unwrap()),
                quote(logical.to_str().unwrap()),
            )
        },
    );
    let wrapper = staged.join("rustx-execution-client");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport PATH={} HOME={home} TMPDIR={scratch}\n{command} \"$@\"\n",
            quote(path),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    staged.join("rustx")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one real-process capability and workspace proof
async fn a_real_child_executes_native_tools_in_the_frozen_nested_workspace() {
    let (handoff, received) = tokio::sync::oneshot::channel();
    let handoff = std::sync::Mutex::new(Some(handoff));
    let server = crate::common::FixtureServer::start_with_body(move |_, _, body| {
        let reply = route_native_tools(body);
        if body.contains("completed; the message that follows") {
            handoff
                .lock()
                .unwrap()
                .take()
                .expect("exactly one Root completion handoff")
                .send(())
                .unwrap();
        }
        reply
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    let workspace = repository.join("nested/session");
    std::fs::create_dir_all(&workspace).unwrap();
    let private = (cfg!(target_os = "linux")
        && std::env::var_os("RUSTX_MOUNT_VIEW_PROOF").is_some())
    .then(|| {
        let physical = root.path().join("client-workspace");
        std::fs::create_dir(&physical).unwrap();
        physical
    });
    let physical = private.as_deref().unwrap_or(&workspace);
    git(&repository, &["init", "-b", "fixture"]);
    git(
        &repository,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    let worktrees = git(&repository, &["worktree", "list", "--porcelain"]);
    let refs = git(&repository, &["show-ref"]);
    let identity = std::fs::metadata(&workspace).unwrap();
    // The optional Agent status plugin has separate follow-up turn coverage.
    // This proof counts exactly the authored Root and child invocations.
    let session = SHARED_SUBDIRECTORY_SESSION_TOML
        .replace(
            "[agent.plugins.agent_status]\nenabled = true",
            "[agent.plugins.agent_status]\nenabled = false",
        )
        .replace(
            "builtin = [\"read\"]",
            "builtin = [\"read\", \"write\", \"edit\", \"bash\"]",
        );
    // The Client starts a real preparation before Bash's FIFO reader
    // completes. Session drain must reap the physical probe.
    let bin = root.path().join("probe-bin");
    std::fs::create_dir(&bin).unwrap();
    let uv = bin.join("uv");
    let witness = root.path().join("python-probe.sock");
    let listener = tokio::net::UnixListener::bind(&witness).unwrap();
    std::fs::write(
        &uv,
        format!(
            r"#!/usr/bin/python3
import os, pathlib, socket, sys
if sys.argv[1] == '--version':
    print('uv 0.10.2')
    sys.exit(0)
pathlib.Path('.python-probe-pid').write_text(str(os.getpid()))
with socket.socket(socket.AF_UNIX) as peer:
    peer.connect({witness:?})
    peer.sendall(b'probe owner\n')
    with open('.python-probe-ready', 'w') as ready:
        ready.write('ready\n')
    with open('.python-probe-stop', 'rb') as gate:
        gate.read()
",
            witness = witness.display().to_string(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).unwrap();
    for gate in [".python-probe-ready", ".python-probe-stop"] {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(physical.join(gate))
                .status()
                .unwrap()
                .success()
        );
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let models = models_json(&server.url("/v1"));
    // The control-plane configuration remains at root. Authored Agent files
    // belong to the same Client mount as the commands that use them.
    write_parent_fixture(root.path(), physical, &models, &session);
    let workflows = physical.join(".agents/workflows");
    std::fs::create_dir(&workflows).unwrap();
    std::fs::write(
        workflows.join("inspect_shared_mount.yaml"),
        SHARED_MOUNT_WORKFLOW,
    )
    .unwrap();
    std::fs::write(
        physical.join("rustx.toml"),
        "[agent]\nworkflows = [\"inspect_shared_mount\"]\n",
    )
    .unwrap();
    std::fs::write(physical.join("root.txt"), b"client-root-read\n").unwrap();
    std::fs::write(physical.join("workflow.txt"), b"client-workflow-read\n").unwrap();
    if private.is_some() {
        assert!(!workspace.join(".agents/agents/explore.toml").exists());
        assert!(
            !workspace
                .join(".agents/workflows/inspect_shared_mount.yaml")
                .exists()
        );
        assert!(!workspace.join("root.txt").exists());
        assert!(!workspace.join("workflow.txt").exists());
    }
    let mut bindings = ParentBindings::under(root.path(), &workspace);
    bindings.execution = true;
    bindings.program = stage_parent_with_client_path(
        root.path(),
        &bindings.home,
        &path,
        private
            .as_deref()
            .map(|physical| (physical, workspace.as_path())),
    );
    let mut process = Parent::spawn(
        &bindings,
        &[("RUSTX_SUBAGENT_TEST_KEY", "native-secret")],
        None,
    )
    .await;
    process.attach().await;
    process.start_turn("ROOT-NATIVE-TOOLS").await;
    tokio::time::timeout(LIVENESS, received)
        .await
        .expect("completion handoff liveness")
        .unwrap();
    let snapshot = tokio::time::timeout(LIVENESS, async {
        loop {
            let snapshot = process.snapshot().await;
            for agent in &snapshot.agents {
                assert!(
                    !agent.activation_state.is_terminal()
                        || agent.activation_state
                            == rustx::runtime::subagent::SubagentState::Succeeded,
                    "child failed: {agent:?}"
                );
            }
            let root_completed = snapshot.attempt.as_ref().is_some_and(|attempt| matches!(
                attempt.phase,
                rustx::runtime_client::snapshot::RuntimeClientAttemptPhase::Settled {
                    outcome: rustx::runtime_client::event::RuntimeClientOutcome::Completed { .. },
                }
            ));
            if root_completed
                && snapshot.agents.iter().any(|agent| {
                    agent.activation_state == rustx::runtime::subagent::SubagentState::Succeeded
                        && agent.state == rustx::runtime::subagent::AgentState::Inactive
                        && agent.current_activation.is_none()
                })
            {
                break snapshot;
            }
        }
    })
    .await
    .expect("child physically settles normally");
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(
        snapshot.agents[0].workspace.canonicalize().unwrap(),
        workspace.canonicalize().unwrap()
    );
    let tools = process.capability_tools().await;
    for control in [
        "subagent",
        "list_agents",
        "send_message",
        "wait_agent",
        "interrupt_agent",
    ] {
        assert_eq!(tools.iter().filter(|tool| tool.name == control).count(), 1);
    }
    assert_eq!(
        std::fs::read_to_string(physical.join("child.txt")).unwrap(),
        "after\n"
    );
    if private.is_some() {
        assert!(!workspace.join("child.txt").exists());
        assert!(!workspace.join(".python-probe-pid").exists());
    }
    assert!(!repository.join("child.txt").exists());
    assert!(!root.path().join("workspaces").exists());
    assert!(!root.path().join("private/workspaces").exists());
    assert_eq!(
        git(&repository, &["worktree", "list", "--porcelain"]),
        worktrees
    );
    assert_eq!(git(&repository, &["show-ref"]), refs);
    assert!(!repository.join(".git/worktrees").exists());
    let after = std::fs::metadata(&workspace).unwrap();
    assert_eq!(
        (after.dev(), after.ino()),
        (identity.dev(), identity.ino()),
        "the binding directory is not replaced"
    );
    let requests = server.request_bodies();
    let sequence: Vec<_> = requests
        .iter()
        .map(|body| {
            let request: Value = serde_json::from_str(body).unwrap();
            let messages = request["messages"].as_array().unwrap();
            let ids: Vec<_> = child_results(&request)
                .iter()
                .map(|result| result["tool_call_id"].as_str().unwrap().to_owned())
                .collect();
            (
                body.contains("ROOT-NATIVE-TOOLS"),
                messages.last().map(|message| message["role"].clone()),
                ids,
            )
        })
        .collect();
    assert_eq!(
        requests.len(),
        10,
        "four Root tool turns, five child turns and one Root completion handoff: {sequence:?}"
    );
    assert_eq!(sequence.iter().filter(|(root, _, _)| *root).count(), 5);
    assert_eq!(
        requests
            .iter()
            .filter(|body| body.contains("completed; the message that follows"))
            .count(),
        1
    );
    let final_child = requests
        .iter()
        .map(|body| serde_json::from_str::<Value>(body).unwrap())
        .find(|request| child_results(request).len() == 4)
        .expect("all four real Tool results reached the child model");
    let ids: Vec<_> = child_results(&final_child)
        .iter()
        .map(|result| result["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["shared-write", "shared-edit", "shared-read", "shared-bash"]
    );
    let (peer, _) = tokio::time::timeout(LIVENESS, listener.accept())
        .await
        .expect("physical probe observation")
        .unwrap();
    let probe_pid = peer.peer_cred().unwrap().pid().map_or_else(
        || {
            std::fs::read_to_string(physical.join(".python-probe-pid"))
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap()
        },
        |pid| u32::try_from(pid).unwrap(),
    );
    let mut peer = tokio::io::BufReader::new(peer);
    let mut received = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut peer, &mut received)
        .await
        .unwrap();
    assert_eq!(received, "probe owner\n");
    assert!(process_state(probe_pid).is_some_and(|state| state != 'Z'));
    let (status, stderr) = process.shutdown().await;
    assert!(status.success(), "clean shutdown: {stderr}");
    assert_eq!(
        tokio::io::AsyncReadExt::read(&mut peer, &mut [0])
            .await
            .unwrap(),
        0
    );
    wait_for_no_running_processes(&[probe_pid]).await;
}
