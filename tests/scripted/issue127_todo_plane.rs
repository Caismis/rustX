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
//!
//! # Why this suite is in-crate
//!
//! Driving the executor directly means standing in for the Agent Loop, and
//! the loop's part of a `todo` call is the batch that owns the mutation.
//! That batch is crate-private on purpose — settling one asserts that the
//! Ledger already carries the list being installed — so the fixture that
//! opens one, [`support::todo::TodoPlane`], is reachable only from here.
//! What the suite *asserts* is published API: the tool's replies, and the
//! committed list through `ConversationToolRuntime::todo_snapshot`.

use super::support::todo::TodoPlane;

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

async fn create(plane: &TodoPlane, subject: &str) -> ToolExecutionResult {
    plane
        .run(serde_json::json!({ "action": "create", "subject": subject }))
        .await
}

#[test]
fn todo_is_registered_as_a_fixed_policy_builtin() {
    let plane = TodoPlane::open();
    let definition = plane
        .fixture
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
    let plane = TodoPlane::open();
    let created = create(&plane, "Write the parser").await;
    assert_eq!(created.status, ToolExecutionStatus::Success);
    assert_eq!(summary(&created), "Created #1: Write the parser (pending)");
    let snapshot = published(&created);
    assert_eq!(snapshot.next_id, 2);
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(snapshot.tasks[0].status, TodoStatus::Pending);

    let started = plane
        .run(serde_json::json!({
            "action": "update",
            "id": 1,
            "status": "in_progress",
            "active_form": "writing the parser",
        }))
        .await;
    assert_eq!(summary(&started), "Updated #1 (pending → in_progress)");

    let repeated = plane
        .run(serde_json::json!({ "action": "update", "id": 1, "status": "in_progress" }))
        .await;
    assert_eq!(
        summary(&repeated),
        "No change: #1 already matches the requested values (status: in_progress)",
        "a model that repeats an update is told it changed nothing"
    );

    create(&plane, "Write the tests").await;
    let listed = plane.run(serde_json::json!({ "action": "list" })).await;
    assert_eq!(
        summary(&listed),
        "[in_progress] #1 Write the parser (writing the parser)\n[pending] #2 Write the tests"
    );
}

#[tokio::test]
async fn a_rejected_call_leaves_the_list_untouched() {
    let plane = TodoPlane::open();
    create(&plane, "Write the parser").await;
    let before = plane.working();

    let unknown = plane
        .run(serde_json::json!({ "action": "update", "id": 9, "status": "completed" }))
        .await;
    assert_eq!(error(&unknown), "#9 not found");
    assert!(
        unknown.content.is_empty(),
        "a rejected call publishes no list, because no mutation happened"
    );

    let empty_update = plane
        .run(serde_json::json!({ "action": "update", "id": 1 }))
        .await;
    assert!(
        error(&empty_update).starts_with("update requires at least one mutable field"),
        "{}",
        error(&empty_update)
    );

    let blank = plane
        .run(serde_json::json!({ "action": "create", "subject": "   " }))
        .await;
    assert_eq!(error(&blank), "subject required for create");

    // An argument outside the contract never reaches the executor at all:
    // the registry rejects it against the generated schema first.
    let outcome = plane
        .fixture
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

    assert_eq!(plane.working(), before);
}

#[tokio::test]
async fn dependencies_are_reported_in_both_directions() {
    let plane = TodoPlane::open();
    create(&plane, "Write the parser").await;
    create(&plane, "Write the tests").await;
    plane
        .run(serde_json::json!({ "action": "update", "id": 2, "add_blocked_by": [1] }))
        .await;

    let blocked = plane
        .run(serde_json::json!({ "action": "get", "id": 2 }))
        .await;
    assert_eq!(
        summary(&blocked),
        "[pending] #2 Write the tests ⛓ #1\nblocked_by: #1"
    );

    let blocking = plane
        .run(serde_json::json!({ "action": "get", "id": 1 }))
        .await;
    assert_eq!(
        summary(&blocking),
        "[pending] #1 Write the parser\nblocks: #2",
        "the reverse edge is derived, never stored"
    );

    let cycle = plane
        .run(serde_json::json!({ "action": "update", "id": 1, "add_blocked_by": [2] }))
        .await;
    assert_eq!(
        error(&cycle),
        "the requested dependency would create a cycle in the blocked_by graph"
    );
}

#[tokio::test]
async fn tombstones_are_hidden_from_the_list_but_never_removed() {
    let plane = TodoPlane::open();
    create(&plane, "Write the parser").await;
    create(&plane, "Write the tests").await;
    let deleted = plane
        .run(serde_json::json!({ "action": "delete", "id": 1 }))
        .await;
    assert_eq!(summary(&deleted), "Deleted #1: Write the parser");
    assert_eq!(
        published(&deleted).tasks.len(),
        2,
        "a tombstone stays in the published list"
    );

    let listed = plane.run(serde_json::json!({ "action": "list" })).await;
    assert_eq!(summary(&listed), "[pending] #2 Write the tests");

    let with_deleted = plane
        .run(serde_json::json!({ "action": "list", "include_deleted": true }))
        .await;
    assert_eq!(
        summary(&with_deleted),
        "[deleted] #1 Write the parser\n[pending] #2 Write the tests"
    );

    let cleared = plane.run(serde_json::json!({ "action": "clear" })).await;
    assert_eq!(summary(&cleared), "Cleared 2 tasks");
    assert_eq!(published(&cleared), TodoSnapshot::empty());
    let empty = plane.run(serde_json::json!({ "action": "list" })).await;
    assert_eq!(summary(&empty), "No tasks");
}

/// Task text is drawn by a terminal client, so the tool is where model
/// output stops being arbitrary bytes: a control character is rejected at
/// the input contract, before any list state is touched.
#[tokio::test]
async fn text_a_terminal_row_cannot_hold_is_rejected_before_anything_is_written() {
    let plane = TodoPlane::open();
    let spoofed = plane
        .run(serde_json::json!({ "action": "create", "subject": "safe\nspoofed" }))
        .await;
    assert_eq!(
        error(&spoofed),
        "subject may not contain the control character U+000A, and it is one line",
    );
    assert_eq!(
        plane.working(),
        TodoSnapshot::empty(),
        "a rejected call writes nothing at all"
    );

    for (field, value) in [
        ("subject", "\u{1b}[31mred"),
        ("active_form", "writing\tfast"),
        ("owner", "me\r"),
        ("subject", "ship\u{202e}suofegnad"),
        // U+061C ARABIC LETTER MARK: a bidi control that is `Cf` rather than
        // a control character, so every rule written as `is_control` lets it
        // through while it reverses reading order like its neighbours.
        ("subject", "ship\u{61c}dangerous"),
        ("active_form", "shipping\u{61c}now"),
        ("owner", "me\u{61c}"),
    ] {
        let rejected = plane
            .run(serde_json::json!({ "action": "create", "subject": "Ship", field: value }))
            .await;
        assert!(
            error(&rejected).starts_with(&format!("{field} may not contain the control character")),
            "{field}: {}",
            error(&rejected)
        );
    }

    // The one long-form field keeps its line breaks, and nothing else.
    let described = plane
        .run(serde_json::json!({
            "action": "create",
            "subject": "Ship",
            "description": "first\nsecond",
        }))
        .await;
    assert_eq!(described.status, ToolExecutionStatus::Success);
}

/// The tool mutates only provisional state: the conversation's authority
/// moves where the Agent Loop commits the batch, never inside an executor.
/// A registry-driven call — this fixture, or any caller holding the public
/// executor — therefore publishes a snapshot without ever moving the list a
/// restart would rebuild, and abandoning the batch takes that snapshot with
/// it.
#[tokio::test]
async fn an_executor_driven_call_publishes_a_list_without_moving_the_authority() {
    let mut plane = TodoPlane::open();
    let created = create(&plane, "Write the parser").await;
    assert_eq!(published(&created).tasks.len(), 1);
    assert_eq!(
        plane.committed(),
        TodoSnapshot::empty(),
        "nothing canonical was published, so the authority did not move"
    );
    assert!(plane.has_staged());

    plane.abandon();
    assert!(!plane.has_staged());
    assert_eq!(plane.working(), TodoSnapshot::empty());
}

/// A `todo` call that belongs to no batch writes nothing.
///
/// The authority is the batch that publishes the mutation, so a dispatch
/// with no batch behind it is refused rather than served from whatever
/// provisional state happens to exist. That is what keeps a call driven
/// beside the Agent Loop out of the snapshot the loop is about to commit.
#[tokio::test]
async fn a_call_that_belongs_to_no_batch_is_refused() {
    let mut plane = TodoPlane::open();
    create(&plane, "Write the parser").await;
    plane.abandon();

    for action in [
        serde_json::json!({ "action": "create", "subject": "Written outside a batch" }),
        serde_json::json!({ "action": "update", "id": 1, "status": "completed" }),
        serde_json::json!({ "action": "list" }),
        serde_json::json!({ "action": "clear" }),
    ] {
        let refused = plane.run(action.clone()).await;
        assert!(
            error(&refused).starts_with("the task list is not open for this call"),
            "{action}: {}",
            error(&refused)
        );
    }
    assert_eq!(
        plane.committed(),
        TodoSnapshot::empty(),
        "a refused call moves neither the authority nor a stage"
    );
    assert!(!plane.has_staged());
}

/// A subject that survives an `update` is a subject a rebuild accepts.
///
/// Blanking one out passed the schema's `minLength` — whitespace is
/// characters — and the control-character rule, so the call settled and
/// published a snapshot the next restart refused to read back.
#[tokio::test]
async fn a_subject_cannot_be_blanked_out_by_an_update() {
    let plane = TodoPlane::open();
    create(&plane, "Write the parser").await;
    let blanked = plane
        .run(serde_json::json!({ "action": "update", "id": 1, "subject": "   " }))
        .await;
    assert_eq!(error(&blanked), "a task subject cannot be blank");
    assert_eq!(
        plane.working().task(1).expect("kept").subject,
        "Write the parser",
        "a rejected update leaves the task exactly as it was"
    );

    let trimmed = plane
        .run(serde_json::json!({ "action": "update", "id": 1, "subject": "  Write the lexer  " }))
        .await;
    assert_eq!(summary(&trimmed), "Updated #1");
    published(&trimmed)
        .validate()
        .expect("every published list is one a rebuild can adopt");
}

/// Metadata is nested and both halves of it are rendered: `get` prints
/// `metadata.<key>: <value>`, so a key reaches a terminal exactly as
/// literally as a subject does.
#[tokio::test]
async fn metadata_keys_and_values_obey_the_same_text_rule() {
    let plane = TodoPlane::open();
    for metadata in [
        serde_json::json!({ "own\u{1b}]0;owned\u{7}er": "me" }),
        serde_json::json!({ "owner": "me\u{202e}dangerous" }),
        serde_json::json!({ "owner": { "nested": "line\nbreak" } }),
        serde_json::json!({ "owner": ["fine", "tab\there"] }),
        // The same U+061C, in each of the three places metadata nests: a
        // key, a value, and a string inside a nested value.
        serde_json::json!({ "own\u{61c}er": "me" }),
        serde_json::json!({ "owner": "me\u{61c}reversed" }),
        serde_json::json!({ "owner": { "nested": "me\u{61c}reversed" } }),
        serde_json::json!({ "owner": ["fine", "me\u{61c}reversed"] }),
    ] {
        let rejected = plane
            .run(serde_json::json!({ "action": "create", "subject": "Ship", "metadata": metadata }))
            .await;
        assert!(
            error(&rejected).starts_with("metadata may not contain the control character"),
            "{}",
            error(&rejected)
        );
    }
    assert_eq!(
        plane.working(),
        TodoSnapshot::empty(),
        "no rejected call wrote anything"
    );

    let accepted = plane
        .run(serde_json::json!({
            "action": "create",
            "subject": "Ship",
            "metadata": { "owner": "me", "size": 3 },
        }))
        .await;
    assert_eq!(accepted.status, ToolExecutionStatus::Success);
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

    let plane = TodoPlane::open();
    create(&plane, "Write the parser").await;
    let committed = plane
        .run(serde_json::json!({ "action": "update", "id": 1, "status": "in_progress" }))
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
        resumed.todo_snapshot(),
        published(&committed),
        "the resumed conversation opens on the list its history last committed"
    );
}
