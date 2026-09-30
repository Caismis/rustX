//! Issue #138 (real process boundary): a launched **named** subagent child
//! inherits the parent runtime's frozen `ModelTimeoutPolicy` and applies it
//! inside its own ordinary Agent Loop — response-start deadline, generic
//! transient retry, bounded failure — while the parent observes exactly one
//! terminal notice and never a retry.
//!
//! This is the one conformance case that must cross the real child process
//! boundary: the frozen policy travels through the typed `SubagentChildSpec`
//! handshake into the real child composition. The named definition is
//! resolved from the invoking resource snapshot and selects both a frozen
//! Builtin and a Skill, so the child crosses the current #144 resolver and
//! #145 child-owned Skill materialization boundary before it reaches the
//! Agent Loop. A fully observed child request reaches a never-released header
//! gate; the inherited 300ms deadline must settle the child before the outer
//! guard, which is below the default 30s response-start timeout. HTTP bodies
//! are transport evidence, not retry cardinality: cancellation may interrupt
//! body transfer. Exact timeout retries belong to the manual-clock test
//! `repeated_runtime_timeouts_use_the_bounded_generic_retry_budget_without_cancellation`.
//! Parent history and subagent state prove terminal uniqueness here.

use std::sync::Arc;

use crate::parent::{Parent, ParentBindings};

/// The outer subprocess conformance guard. The inherited 300ms response-start
/// policy completes the scripted run in roughly 16s (2s + 4s + 8s of real
/// retry backoff plus four short deadlines). It is deliberately below the
/// default 30s response-start timeout: if child composition re-defaults the
/// policy, the first gated request cannot settle before this guard expires.
const LIVENESS: std::time::Duration = std::time::Duration::from_secs(27);

/// A catalog pointing at the local fixture server.
fn models_json(base_url: &str) -> String {
    format!(
        r#"[providers.fixture]
base_url = "{base_url}"
api_key = "$RUSTX_SUBAGENT_TEST_KEY"

[models."fixture/subagent-model"]
provider = "fixture"
id = "subagent-model"
protocol = "openai_chat_completions"
context_window = 128000
max_output_tokens = 512

[models."fixture/subagent-model".capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
tool_calls = true
reasoning = false

[models."fixture/subagent-model".compat]
chat_reasoning_replay = "omit"
"#
    )
}

/// The launch configuration with the deliberately tiny frozen timeout
/// policy that every launched child must inherit.
const SESSION_TOML: &str = r#"agent_id = "agent-parent"

[context]
reserve_tokens = 1024
keep_recent_tokens = 8192


[model_timeout_policy]
response_start_timeout_ms = 300
stream_idle_timeout_ms = 300


[subagents]
max_concurrent = 4

[subagents.roles]
[subagents.roles.conformance]
description = "Issue 138 named conformance child."
skills = ["conformance"]

[subagents.roles.conformance.tools]
builtin = ["read"]


[agent]
agents = ["conformance"]

[agent.model]
model = "fixture/subagent-model"


[agent.tools]
builtin = ["read"]
"#;

/// Writes the named child definition and Skill, then launches the real App
/// Server parent over a new Session in `<root>/workspace`.
async fn spawn_parent(root: &std::path::Path, models: &str, session: &str, key: &str) -> Parent {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(workspace.join(".agents/agents/conformance"))
        .expect("subagent resources");
    std::fs::write(
        workspace.join(".agents/agents/conformance.toml"),
        "Execute the delegated conformance task exactly as requested.\n",
    )
    .expect("subagent instructions");
    let skill = workspace.join(".agents/skills/conformance");
    std::fs::create_dir_all(&skill).expect("skill package");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: conformance\ndescription: Issue 138 conformance skill.\n---\n\nUse the ordinary child runtime.\n",
    )
    .expect("skill manifest");
    let mut document: serde_json::Value = rustx::toml_authoring::parse(session.as_bytes()).unwrap();
    crate::launch_fixture::write_roles(&workspace, &mut document["subagents"]);
    std::fs::write(
        root.join("rustx.toml"),
        format!("{}\n{models}", toml::to_string_pretty(&document).unwrap()),
    )
    .expect("rustx.toml");
    Parent::spawn(
        &ParentBindings::under(root, &workspace),
        &[("RUSTX_SUBAGENT_TEST_KEY", key)],
        None,
    )
    .await
}

/// Routes one provider request by body content. The parent and the child
/// runtime share this fixture, so attempt order is NOT the routing key.
/// Every child request hits the gate that is never released; the child's
/// inherited response-start deadline is what ends each attempt request.
fn route(body: &str, gate: &Arc<crate::common::HeaderGate>) -> crate::common::FixtureReply {
    let has_tool_history =
        body.contains("\\\"role\\\":\\\"tool\\\"") || body.contains("\"role\":\"tool\"");
    if body.contains("count the workspace files") && !body.contains("please delegate") {
        // The child runtime's own model call: the delegated task is its
        // user message. The response headers never arrive.
        crate::common::sse_fixture("openai_chat", "subagent_child_answer.sse")
            .with_header_gate(Arc::clone(gate))
    } else if has_tool_history {
        // The parent's turn after the child's terminal notice arrived.
        crate::common::sse_fixture("openai_chat", "plain_text.sse")
    } else {
        // The parent's first turn: delegate.
        crate::common::sse_fixture("openai_chat", "issue138_subagent_tool_call.sse")
    }
}

/// The child inherits the frozen timeout policy, times out on the gated
/// provider response, and fails within a bound impossible with default policy.
/// The deterministic Agent Loop suite owns exact retry cardinality. The parent receives
/// exactly one Runtime-authored failure notice with a timeout diagnostic
/// and consumes it in an ordinary continuation turn. Nothing about the
/// retries — ordinals, delays, a "retrying" state — is parent-visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_child_inherits_the_frozen_timeout_policy_and_retries_locally() {
    tokio::time::timeout(
        LIVENESS,
        run_real_child_inherits_the_frozen_timeout_policy_and_retries_locally(),
    )
    .await
    .expect(
        "the inherited 300ms child timeout must settle within 27s; a default 30s timeout cannot",
    );
}

#[allow(clippy::too_many_lines)]
async fn run_real_child_inherits_the_frozen_timeout_policy_and_retries_locally() {
    let gate = crate::common::HeaderGate::new();
    let gate_for_server = Arc::clone(&gate);
    let server = crate::common::FixtureServer::start_with_body(move |_attempt, _head, body| {
        route(body, &gate_for_server)
    })
    .await;
    let root = tempfile::tempdir().expect("temp root");
    let mut process = spawn_parent(
        root.path(),
        &models_json(&server.url("/v1")),
        SESSION_TOML,
        "subagent-secret",
    )
    .await;

    process.attach().await;
    process.start_turn("please delegate").await;

    // The child request reaching the never-released gate is the
    // deterministic frontier: from here on, only the child's inherited
    // deadline can move the run forward.
    tokio::time::timeout(LIVENESS, gate.wait_entered())
        .await
        .expect("the child must reach the gated provider response");

    // Poll the authoritative snapshot until the child settles Failed and
    // the parent consumed the notice in an ordinary continuation turn.
    let mut final_snapshot = None;
    for _ in 0..8_000 {
        let snapshot = process.snapshot().await;
        let failed = snapshot.agents.iter().any(|subagent| {
            subagent.activation_state == rustx::runtime::subagent::SubagentState::Failed
        });
        let notice = snapshot.messages.iter().any(|message| match message {
            rustx::message::types::MessageBlock::User(user) => {
                matches!(user.source, rustx::message::types::UserSource::Runtime)
                    && user.content.iter().any(|block| match block {
                        rustx::message::types::UserContentBlock::Text(text) => {
                            text.text.contains("failed")
                        }
                        _ => false,
                    })
            }
            _ => false,
        });
        let parent_consumed = snapshot.messages.iter().any(|message| match message {
            rustx::message::types::MessageBlock::Assistant(assistant) => {
                assistant.content.iter().any(|block| match block {
                    rustx::message::types::AssistantContentBlock::Text(text) => {
                        text.text.contains("Hello world")
                    }
                    _ => false,
                })
            }
            _ => false,
        });
        if failed && notice && parent_consumed {
            final_snapshot = Some(snapshot);
            break;
        }
        tokio::task::yield_now().await;
    }
    let Some(snapshot) = final_snapshot else {
        let _ = process.shutdown().await;
        panic!("the child timeout/retry chain must settle within the liveness guard");
    };

    // Exactly one child, Failed, carrying the bounded timeout diagnostic.
    assert_eq!(snapshot.agents.len(), 1, "exactly one child was owned");
    let subagent = &snapshot.agents[0];
    assert_eq!(
        subagent.activation_state,
        rustx::runtime::subagent::SubagentState::Failed
    );
    let detail = subagent.detail.clone().expect("the terminal detail");
    assert!(
        detail.contains("Timeout") || detail.contains("timed out"),
        "the bounded diagnostic is the child's own deadline outcome: {detail}"
    );

    // Exactly one Runtime-authored failure notice exists, and it contains
    // no child publication content (the child never produced any: every
    // request died at the response-start frontier).
    let notices: Vec<_> = snapshot
        .messages
        .iter()
        .filter(|message| match message {
            rustx::message::types::MessageBlock::User(user) => {
                matches!(user.source, rustx::message::types::UserSource::Runtime)
                    && user.content.iter().any(|block| match block {
                        rustx::message::types::UserContentBlock::Text(text) => {
                            text.text.contains("failed")
                        }
                        _ => false,
                    })
            }
            _ => false,
        })
        .collect();
    assert_eq!(
        notices.len(),
        1,
        "exactly one parent-facing terminal notice"
    );
    let notice_json = serde_json::to_string(&notices[0]).expect("notice json");
    assert!(
        !notice_json.contains("CHILD-ANSWER"),
        "no child publication content crosses: {notice_json}"
    );

    // HeaderGate is entered only after one complete child body is recorded.
    // That body proves a genuine provider invocation with child materialization,
    // not how many Agent Loop attempts occurred. Deadlines may abort other
    // connections before the fixture records their complete bodies.
    let bodies = server.request_bodies();
    let first_child_request = bodies
        .iter()
        .find(|body| {
            body.contains("count the workspace files") && !body.contains("please delegate")
        })
        .expect("the child request body");
    assert!(
        first_child_request.contains("Workspace Skill root")
            && first_child_request.contains("skills/workspace")
            && first_child_request.contains("<name>conformance</name>"),
        "the named child request uses the captured roots and progressive disclosure metadata: {first_child_request}"
    );
    // Count committed delegation proposals, not HTTP sends that a deadline
    // could interrupt. Together with the single owned child this proves the
    // parent never reissues/relaunches the delegated work.
    let parent_delegations = snapshot.messages.iter().filter_map(|message| match message {
        rustx::message::types::MessageBlock::Assistant(assistant) => Some(&assistant.content),
        _ => None,
    }).flatten().filter(|block| matches!(block,
        rustx::message::types::AssistantContentBlock::ToolCall(call) if call.name == "subagent"
    )).count();
    assert_eq!(parent_delegations, 1, "one authoritative parent delegation");
    assert!(
        !serde_json::to_string(&snapshot.messages)
            .unwrap()
            .contains("CHILD-ANSWER")
    );

    // Release the gate so the fixture server's held handlers can finish,
    // then shut the parent down cleanly.
    gate.release();
    let (status, stderr) = process.shutdown().await;
    assert!(
        status.success(),
        "the process must exit cleanly: {status} stderr={stderr}"
    );
}
