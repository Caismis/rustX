//! Deterministic coverage of the native `todo` tool and the conversation's
//! task list.
//!
//! The tool is registered like any other native capability, so these tests
//! drive it through the ordinary registry preflight and executor boundary.
//! Three properties matter and are covered here:
//!
//! - every settled call publishes the complete post-call list, which is what
//!   makes the list rebuildable without a second persistence path;
//! - a rejected call is an ordinary failed tool result that leaves the list
//!   exactly as it was;
//! - a tool runtime constructed over existing conversation history opens on
//!   the list that history last committed.

mod common;

use common::{NativeFixture, native_fixture, run_tool};
use rustx::tools::todo::{TodoSnapshot, TodoStatus};
use rustx::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy, ToolExecutionResult,
    ToolExecutionStatus, ToolOrigin, ToolResultContent,
};

fn summary(result: &ToolExecutionResult) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            ToolResultContent::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("a settled todo result summarizes what it did")
}

fn published(result: &ToolExecutionResult) -> TodoSnapshot {
    let value = result
        .content
        .iter()
        .find_map(|content| match content {
            ToolResultContent::Json { value } => Some(value.clone()),
            _ => None,
        })
        .expect("a settled todo result publishes the whole list");
    serde_json::from_value(value).expect("the published list is a snapshot")
}

fn error(result: &ToolExecutionResult) -> String {
    match &result.status {
        ToolExecutionStatus::Failed { error } => error.clone(),
        status => panic!("expected a rejected call, got {status:?}"),
    }
}

async fn create(fixture: &NativeFixture, subject: &str) -> ToolExecutionResult {
    run_tool(
        fixture,
        "todo",
        serde_json::json!({ "action": "create", "subject": subject }),
    )
    .await
}

#[test]
fn todo_is_registered_as_a_fixed_policy_builtin() {
    let fixture = native_fixture();
    let definition = fixture
        .registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "todo")
        .expect("todo is registered");
    assert_eq!(definition.id.as_str(), rustx::tools::todo::TODO_TOOL_ID);
    assert_eq!(definition.origin, ToolOrigin::Builtin);
    assert_eq!(
        definition.execution_policy,
        ToolExecutionPolicy::ForegroundOnly,
        "one list cannot be mutated by a detached execution"
    );
    assert_eq!(
        definition.concurrency_policy,
        ToolConcurrencyPolicy::Sequential,
        "two concurrent mutations would publish racing snapshots"
    );
    assert_eq!(definition.approval_policy, ToolApprovalPolicy::Never);
    assert_eq!(definition.input_schema["type"], "object");
    assert_eq!(definition.input_schema["additionalProperties"], false);
    assert_eq!(
        definition.input_schema["required"],
        serde_json::json!(["action"]),
        "only the action is universally required; per-action requirements are semantic"
    );
}

#[tokio::test]
async fn every_settled_call_publishes_the_complete_list() {
    let fixture = native_fixture();
    let created = create(&fixture, "Write the parser").await;
    assert_eq!(created.status, ToolExecutionStatus::Success);
    assert_eq!(summary(&created), "Created #1: Write the parser (pending)");
    let snapshot = published(&created);
    assert_eq!(snapshot.next_id, 2);
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].status, TodoStatus::Pending);

    let started = run_tool(
        &fixture,
        "todo",
        serde_json::json!({
            "action": "update",
            "id": 1,
            "status": "in_progress",
            "active_form": "writing the parser",
        }),
    )
    .await;
    assert_eq!(summary(&started), "Updated #1 (pending → in_progress)");

    let repeated = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "update", "id": 1, "status": "in_progress" }),
    )
    .await;
    assert_eq!(
        summary(&repeated),
        "No change: #1 already matches the requested values (status: in_progress)",
        "a model that repeats an update is told it changed nothing"
    );

    create(&fixture, "Write the tests").await;
    let listed = run_tool(&fixture, "todo", serde_json::json!({ "action": "list" })).await;
    assert_eq!(
        summary(&listed),
        "[in_progress] #1 Write the parser (writing the parser)\n[pending] #2 Write the tests"
    );
}

#[tokio::test]
async fn a_rejected_call_leaves_the_list_untouched() {
    let fixture = native_fixture();
    create(&fixture, "Write the parser").await;
    let before = fixture.runtime.todos().snapshot();

    let unknown = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "update", "id": 9, "status": "completed" }),
    )
    .await;
    assert_eq!(error(&unknown), "#9 not found");
    assert!(
        unknown.content.is_empty(),
        "a rejected call publishes no list, because no mutation happened"
    );

    let empty_update = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "update", "id": 1 }),
    )
    .await;
    assert!(
        error(&empty_update).starts_with("update requires at least one mutable field"),
        "{}",
        error(&empty_update)
    );

    let blank = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "create", "subject": "   " }),
    )
    .await;
    assert_eq!(error(&blank), "subject required for create");

    // An argument outside the contract never reaches the executor at all:
    // the registry rejects it against the generated schema first.
    let outcome = fixture
        .registry
        .preflight(&rustx::tools::types::ToolCall {
            id: rustx::runtime::identity::ToolCallId::new("call-unknown-field"),
            tool_id: rustx::runtime::identity::ToolId::new(rustx::tools::todo::TODO_TOOL_ID),
            name: "todo".to_owned(),
            arguments: serde_json::json!({
                "action": "create",
                "subject": "Ship",
                "priority": "high",
            }),
        })
        .expect("identity resolves");
    let rustx::tools::executor::PreflightOutcome::Rejected { error, .. } = outcome else {
        panic!("an undeclared argument is rejected before dispatch");
    };
    assert!(error.contains("priority"), "{error}");

    assert_eq!(fixture.runtime.todos().snapshot(), before);
}

#[tokio::test]
async fn dependencies_are_reported_in_both_directions() {
    let fixture = native_fixture();
    create(&fixture, "Write the parser").await;
    create(&fixture, "Write the tests").await;
    run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "update", "id": 2, "add_blocked_by": [1] }),
    )
    .await;

    let blocked = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "get", "id": 2 }),
    )
    .await;
    assert_eq!(
        summary(&blocked),
        "[pending] #2 Write the tests ⛓ #1\nblocked_by: #1"
    );

    let blocking = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "get", "id": 1 }),
    )
    .await;
    assert_eq!(
        summary(&blocking),
        "[pending] #1 Write the parser\nblocks: #2",
        "the reverse edge is derived, never stored"
    );

    let cycle = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "update", "id": 1, "add_blocked_by": [2] }),
    )
    .await;
    assert_eq!(
        error(&cycle),
        "the requested dependency would create a cycle in the blocked_by graph"
    );
}

#[tokio::test]
async fn tombstones_are_hidden_from_the_list_but_never_removed() {
    let fixture = native_fixture();
    create(&fixture, "Write the parser").await;
    create(&fixture, "Write the tests").await;
    let deleted = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "delete", "id": 1 }),
    )
    .await;
    assert_eq!(summary(&deleted), "Deleted #1: Write the parser");
    assert_eq!(
        published(&deleted).tasks.len(),
        2,
        "a tombstone stays in the published list"
    );

    let listed = run_tool(&fixture, "todo", serde_json::json!({ "action": "list" })).await;
    assert_eq!(summary(&listed), "[pending] #2 Write the tests");

    let with_deleted = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "list", "include_deleted": true }),
    )
    .await;
    assert_eq!(
        summary(&with_deleted),
        "[deleted] #1 Write the parser\n[pending] #2 Write the tests"
    );

    let cleared = run_tool(&fixture, "todo", serde_json::json!({ "action": "clear" })).await;
    assert_eq!(summary(&cleared), "Cleared 2 tasks");
    assert_eq!(published(&cleared), TodoSnapshot::empty());
    let empty = run_tool(&fixture, "todo", serde_json::json!({ "action": "list" })).await;
    assert_eq!(summary(&empty), "No tasks");
}

/// A new tool runtime over existing conversation history opens on the list
/// that history last committed — the property that makes the list survive a
/// restart, a resume, and a compaction without any storage of its own.
#[tokio::test]
async fn a_new_tool_runtime_rebuilds_the_list_from_conversation_history() {
    use rustx::durable::{ConversationStore, ConversationStoreBinding, SqliteConversationStore};
    use rustx::message::types::{MessageBlock, ToolMessageBlock};
    use rustx::runtime::identity::{ConversationId, MessageId, ToolCallId, ToolId};
    use rustx::tools::runtime::{ConversationRuntimeConfig, ConversationToolRuntime};

    let fixture = native_fixture();
    create(&fixture, "Write the parser").await;
    let committed = run_tool(
        &fixture,
        "todo",
        serde_json::json!({ "action": "update", "id": 1, "status": "in_progress" }),
    )
    .await;

    // The canonical record of the two calls, exactly as the runtime commits
    // them: the second result carries the whole list, so it alone decides
    // what a later runtime opens on.
    let history = vec![MessageBlock::Tool(ToolMessageBlock {
        id: MessageId::new("message-todo"),
        tool_call_id: ToolCallId::new("call-todo"),
        tool_id: ToolId::new(rustx::tools::todo::TODO_TOOL_ID),
        result: committed.clone(),
    })];

    let dir = tempfile::tempdir().expect("temporary conversation");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(dir.path().join("artifacts")).expect("artifacts");
    let conversation_id = ConversationId::new("conv-resumed");
    let store = std::sync::Arc::new(
        SqliteConversationStore::open(
            conversation_id.clone(),
            &dir.path().join("artifacts/conversation.sqlite"),
        )
        .expect("durable store"),
    );
    store.initialize(&history).expect("history");

    let resumed = ConversationToolRuntime::from_config(
        conversation_id,
        ConversationRuntimeConfig {
            durable_binding: Some(ConversationStoreBinding::new(store)),
            ..ConversationRuntimeConfig::new(&workspace, dir.path().join("artifacts"))
        },
    )
    .expect("tool runtime");

    assert_eq!(
        resumed.todos().snapshot(),
        published(&committed),
        "the resumed conversation opens on the list its history last committed"
    );
}
