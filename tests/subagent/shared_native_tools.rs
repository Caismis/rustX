//! Real child native mutations against a nested, frozen Session binding.
use super::*;
use serde_json::{Value, json};
use std::os::unix::fs::MetadataExt;

fn tool_reply(name: &str, arguments: &Value) -> crate::common::FixtureReply {
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
            crate::common::sse_fixture("openai_chat", "plain_text.sse")
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
            assert!(
                results[0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Successfully wrote")
            );
            tool_reply(
                "edit",
                &json!({"path":"child.txt", "edits":[{"oldText":"before", "newText":"after"}]}),
            )
        }
        2 => {
            assert!(
                results[1]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("Successfully replaced")
            );
            tool_reply("read", &json!({"path":"child.txt"}))
        }
        3 => {
            assert!(
                results[2]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("after")
            );
            tool_reply(
                "bash",
                &json!({"command":"test \"$(cat child.txt)\" = after && printf 'NATIVE-BASH-OK\\n'", "execution_mode":"foreground"}),
            )
        }
        4 => {
            assert!(
                results[3]["content"][0]["text"]
                    .as_str()
                    .unwrap()
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // one real-process capability and workspace proof
async fn a_real_child_executes_native_tools_in_the_frozen_nested_workspace() {
    let server =
        crate::common::FixtureServer::start_with_body(|_, _, body| route_native_tools(body)).await;
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    let workspace = repository.join("nested/session");
    std::fs::create_dir_all(&workspace).unwrap();
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
    let session = format!("approval_mode = \"full_access\"\n{SHARED_SUBDIRECTORY_SESSION_TOML}")
        .replace(
            "builtin = [\"read\"]",
            "builtin = [\"read\", \"write\", \"edit\", \"bash\"]",
        );
    let models = models_json(&server.url("/v1"));
    let mut process = launch_parent(
        root.path(),
        &workspace,
        &models,
        &session,
        "native-secret",
        None,
        false,
    )
    .await;
    process.attach().await;
    process.start_turn("ROOT-NATIVE-TOOLS").await;
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
            if snapshot.agents.iter().any(|agent| {
                agent.activation_state == rustx::runtime::subagent::SubagentState::Succeeded
                    && agent.state == rustx::runtime::subagent::AgentState::Inactive
                    && agent.current_activation.is_none()
            }) {
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
        std::fs::read_to_string(workspace.join("child.txt")).unwrap(),
        "after\n"
    );
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
    let (status, stderr) = process.shutdown().await;
    assert!(status.success(), "clean shutdown: {stderr}");
}
