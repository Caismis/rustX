//! The conversation task list settles exactly where its results become
//! canonical (Issue #127).
//!
//! The list has no persistence of its own: the snapshot each settled `todo`
//! result publishes *is* the durable record. That equivalence only holds if
//! the in-memory list can never run ahead of the Ledger, so a `todo` call
//! writes staged state and the Agent Loop installs it at the one atomic
//! `ToolResult` batch commit.
//!
//! These tests pin both sides of that point through the real Agent Loop:
//! a batch that commits moves the list to exactly what history now says, and
//! a batch that never becomes canonical leaves the list exactly as it was.

use super::{common, support};

use std::sync::Arc;

use rustx::agent::{AgentCancellation, AgentExecution, AgentExecutionRequest};
use rustx::events::types::AttemptOutcome;
use rustx::message::types::{MessageBlock, UserContentBlock, UserMessageBlock, UserSource};
use rustx::model::event::ModelEvent;
use rustx::model::finish::ModelFinishReason;
use rustx::runtime::identity::{AgentId, AttemptId, ConversationId, MessageId};
use rustx::runtime::types::CancellationReason;
use rustx::tools::todo::{TODO_TOOL_ID, TodoSnapshot, TodoStatus};

fn create(id: &'static str, subject: &'static str) -> support::fake::ScriptedCall {
    support::fake::ScriptedCall {
        id,
        tool_id: TODO_TOOL_ID,
        name: "todo",
        arguments: serde_json::json!({ "action": "create", "subject": subject }),
    }
}

/// One assistant turn carrying `calls`, then a plain text turn.
fn turn(calls: &[support::fake::ScriptedCall]) -> Vec<Vec<support::fake::FakeStep>> {
    let mut first = vec![support::fake::FakeStep::Emit(ModelEvent::Started)];
    for (index, call) in calls.iter().enumerate() {
        first.extend(
            support::fake::tool_call_events(u32::try_from(index).expect("block index"), call)
                .into_iter()
                .map(support::fake::FakeStep::Emit),
        );
    }
    first.push(support::fake::FakeStep::Emit(ModelEvent::Completed {
        finish_reason: ModelFinishReason::ToolCalls,
        usage: None,
    }));
    vec![
        first,
        vec![
            support::fake::FakeStep::Emit(ModelEvent::Started),
            support::fake::FakeStep::Emit(ModelEvent::TextDelta {
                block_index: rustx::message::types::ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            support::fake::FakeStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ],
    ]
}

fn request(
    model: &Arc<support::fake::FakeModel>,
    conversation_id: &ConversationId,
) -> AgentExecutionRequest {
    AgentExecutionRequest {
        agent_id: AgentId::new("agent-todo-transaction"),
        conversation_id: conversation_id.clone(),
        attempt_id: AttemptId::new("attempt-todo-transaction"),
        conversation: rustx::conversation::ConversationState::from_messages(vec![
            MessageBlock::User(UserMessageBlock {
                id: MessageId::new("message-todo-transaction"),
                content: vec![UserContentBlock::Text(rustx::message::content::TextBlock {
                    text: "plan the work".to_owned(),
                })],
                source: UserSource::Human,
                kind: rustx::message::types::InboundKind::Message,
                timestamp: None,
            }),
        ])
        .expect("bootstrap conversation"),
        initial_turn_trigger: rustx::agent::InitialTurnTrigger::Continuation,
        timezone: None,
        model: support::attempt_model(model.clone(), "todo-transaction-model"),
    }
}

fn context_runtime(model: &Arc<support::fake::FakeModel>) -> rustx::context::ContextRuntime {
    rustx::context::ContextRuntime::for_attempt(
        rustx::context::SessionContextPolicy {
            reserve_tokens: 0,
            keep_recent_tokens: 0,
            summary_output_cap: None,
        },
        Arc::new(rustx::context::DefaultTokenEstimator),
        rustx::context::AgentStatusComposer::default(),
        &support::attempt_model(model.clone(), "todo-transaction-model"),
    )
    .expect("context runtime")
}

async fn run(
    fixture: &common::NativeFixture,
    calls: &[support::fake::ScriptedCall],
) -> common::DurableExecutionAudit {
    let model = support::fake::fake_model(turn(calls));
    let capability = common::capability_lease(fixture.registry.clone(), &fixture.runtime).await;
    let (lease, coordinator) = capability.into_lease_and_coordinator();
    let cancellation = AgentCancellation::new(CancellationReason::UserRequested);
    let result = AgentExecution::new(
        request(&model, fixture.runtime.conversation_id()),
        lease,
        &cancellation,
        context_runtime(&model),
        &fixture.runtime,
        rustx::agent::AttemptLifecycle::inert(),
    )
    .expect("conversation identity matches the tool runtime")
    .run()
    .await;
    let audit = common::durable_agent_result(result, fixture.store.as_ref());
    drop(coordinator);
    audit
}

/// The snapshot the canonical history of `audit` last published, if any.
fn canonical_list(audit: &common::DurableExecutionAudit) -> Option<TodoSnapshot> {
    audit
        .messages()
        .iter()
        .rev()
        .find_map(rustx::tools::todo::published_snapshot)
        .map(|published| published.expect("the runtime publishes a usable list"))
}

/// A committed batch moves the list to exactly what canonical history says,
/// and leaves nothing staged behind it.
#[tokio::test]
async fn a_committed_batch_settles_the_list_on_its_own_published_snapshot() {
    let fixture = common::native_fixture();
    let audit = run(&fixture, &[create("call-todo-a", "Write the parser")]).await;
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));

    let canonical = canonical_list(&audit).expect("the batch committed a todo result");
    assert_eq!(canonical.tasks.len(), 1);
    assert_eq!(canonical.tasks[0].subject, "Write the parser");
    assert_eq!(
        fixture.runtime.todos().committed(),
        canonical,
        "the conversation's list is exactly the list its history published"
    );
    assert!(
        !fixture.runtime.todos().has_staged(),
        "a committed batch leaves nothing provisional behind"
    );
}

/// Two `todo` calls in one batch compose: the second reads what the first
/// staged, and both settle together on the last published snapshot.
#[tokio::test]
async fn later_calls_of_one_batch_see_what_earlier_ones_staged() {
    let fixture = common::native_fixture();
    let audit = run(
        &fixture,
        &[
            create("call-todo-a", "Write the parser"),
            create("call-todo-b", "Write the tests"),
        ],
    )
    .await;
    assert!(matches!(audit.outcome, AttemptOutcome::Completed { .. }));

    let committed = fixture.runtime.todos().committed();
    assert_eq!(
        committed
            .tasks
            .iter()
            .map(|task| (task.id, task.subject.as_str(), task.status))
            .collect::<Vec<_>>(),
        vec![
            (1, "Write the parser", TodoStatus::Pending),
            (2, "Write the tests", TodoStatus::Pending),
        ],
        "the second create allocated on top of the first one's staged list"
    );
    assert_eq!(canonical_list(&audit).expect("published"), committed);
}

/// A batch that never becomes canonical leaves the list exactly as it was.
///
/// The failure is produced at the batch commit itself: two calls sharing one
/// canonical call id derive one `MessageId` for both results, so the second
/// `prepare_commit` rejects the batch after both executors have already run.
/// Nothing is appended, and the list must not have moved either — otherwise
/// the process would hold tasks that no committed result ever published, and
/// a restart would silently lose them.
#[tokio::test]
async fn a_batch_that_never_becomes_canonical_leaves_the_list_untouched() {
    let fixture = common::native_fixture();
    let before = fixture.runtime.todos().committed();
    assert_eq!(before, TodoSnapshot::empty());

    let audit = run(
        &fixture,
        &[
            create("call-todo-clash", "Write the parser"),
            create("call-todo-clash", "Write the tests"),
        ],
    )
    .await;

    assert!(
        matches!(audit.outcome, AttemptOutcome::Failed { .. }),
        "the duplicated canonical identity fails the batch: {:?}",
        audit.outcome
    );
    assert!(
        canonical_list(&audit).is_none(),
        "no tool result became canonical, so no list was ever published"
    );
    assert_eq!(
        fixture.runtime.todos().committed(),
        before,
        "the authority a restart would rebuild is the authority this process holds"
    );
    assert!(
        !fixture.runtime.todos().has_staged(),
        "the failed batch's provisional list was discarded, not left to leak into the next one"
    );
}

// ---------------------------------------------------------------------------
// The client projection of the same fact
// ---------------------------------------------------------------------------

/// One canonical `todo` result carrying `snapshot`.
fn published_result(id: &str, snapshot: &TodoSnapshot) -> MessageBlock {
    MessageBlock::Tool(rustx::message::types::ToolMessageBlock {
        id: MessageId::new(id),
        tool_call_id: rustx::runtime::identity::ToolCallId::new(format!("call-{id}")),
        tool_id: rustx::runtime::identity::ToolId::new(TODO_TOOL_ID),
        result: rustx::tools::types::ToolExecutionResult {
            status: rustx::tools::types::ToolExecutionStatus::Success,
            content: vec![
                rustx::tools::types::ToolResultContent::Text(rustx::message::content::TextBlock {
                    text: "Created #1: Write the parser (pending)".to_owned(),
                }),
                rustx::tools::types::ToolResultContent::Json {
                    value: serde_json::to_value(snapshot).expect("snapshot"),
                },
            ],
            duration_ms: 0,
            exit_code: None,
            artifacts: Vec::new(),
            truncation: None,
            managed_output: None,
        },
    })
}

fn assistant(id: &str, text: &str) -> MessageBlock {
    MessageBlock::Assistant(rustx::message::types::AssistantMessageBlock {
        id: MessageId::new(id),
        content: vec![rustx::message::types::AssistantContentBlock::Text(
            rustx::message::content::TextBlock {
                text: text.to_owned(),
            },
        )],
    })
}

/// A client attaches to the list the *runtime* derived, not to whatever its
/// own bounded transcript page happens to contain.
///
/// The bootstrap page is the newest 64 transcript items. A conversation that
/// committed a page or more of messages since its last `todo` result would
/// hand a transcript-scanning client no list at all, while the runtime —
/// which rebuilt the list from the whole Ledger — still had one. The panel
/// would vanish and `/todos` would report no tasks, purely because of how far
/// back the result now sits.
#[tokio::test]
async fn a_fresh_attach_carries_the_list_even_when_the_result_is_off_the_page() {
    let list = TodoSnapshot {
        tasks: vec![rustx::tools::todo::TodoTask {
            id: 1,
            subject: "Write the parser".to_owned(),
            description: None,
            active_form: Some("writing the parser".to_owned()),
            status: TodoStatus::InProgress,
            blocked_by: Vec::new(),
            owner: None,
            metadata: None,
        }],
        next_id: 2,
    };
    let mut history = vec![published_result("message-todo", &list)];
    history
        .extend((0..70).map(|index| assistant(&format!("message-after-{index}"), "kept working")));

    let fixture = support::runtime_client_fixture::RuntimeClientFixture::builder("conv-todo-page")
        .durable_history(history)
        .build()
        .await;
    let (snapshot, _) = fixture.host.snapshot().expect("snapshot");

    assert!(
        !snapshot.transcript.entries.iter().any(|entry| matches!(
            &entry.item,
            rustx::runtime_client::RuntimeClientTranscriptItem::Message {
                message: MessageBlock::Tool(_)
            }
        )),
        "the bounded page really has scrolled past the todo result"
    );
    assert_eq!(
        snapshot.todos, list,
        "the client attaches to the list canonical history holds"
    );
}

/// A conversation that never called `todo` attaches to the empty list — a
/// fact, not an absence.
#[tokio::test]
async fn a_conversation_without_a_list_attaches_to_the_empty_one() {
    let fixture = support::runtime_client_fixture::RuntimeClientFixture::builder("conv-todo-none")
        .build()
        .await;
    let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
    assert_eq!(snapshot.todos, TodoSnapshot::empty());
}

/// While the client is live, the list follows the committed result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_projection_follows_a_committed_todo_result() {
    let fixture = support::runtime_client_fixture::RuntimeClientFixture::builder("conv-todo-live")
        .native_tools()
        .scripts(turn(&[create("call-todo-live", "Write the parser")]))
        .build()
        .await;
    let (attachment, _) = fixture
        .host
        .attach(rustx::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
        .expect("attach");
    let subscription = attachment
        .subscribe_events(rustx::runtime_client::RuntimeClientCursor::new(0))
        .expect("subscribe");
    attachment.handle_request(rustx::runtime_client::RuntimeClientRequest::SubmitInbound {
        id: rustx::runtime_client::RequestId::new(1),
        content: vec![rustx::message::types::UserContentBlock::Text(
            rustx::message::content::TextBlock {
                text: "plan the work".to_owned(),
            },
        )],
    });
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let rustx::runtime_client::EventDelivery::Event(event) = subscription.next().await
            else {
                panic!("the subscription must stay open");
            };
            if matches!(
                event.event,
                rustx::runtime_client::RuntimeClientEvent::AttemptSettled { .. }
            ) {
                return;
            }
        }
    })
    .await
    .expect("the attempt settles");

    let (snapshot, _) = fixture.host.snapshot().expect("snapshot");
    assert_eq!(snapshot.todos.tasks.len(), 1);
    assert_eq!(snapshot.todos.tasks[0].subject, "Write the parser");
    assert_eq!(
        snapshot.todos,
        fixture.runtime.tool_runtime().todos().committed(),
        "the client projection and the runtime authority are one derivation"
    );
}
